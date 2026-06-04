import asyncio
import json
import logging
import base64
import time
import os
import uuid
import hashlib
from contextlib import asynccontextmanager
from urllib.parse import quote, urljoin
from urllib import request as urllib_request
from typing import Optional, List, Dict, Any
from fastapi import FastAPI, Request, HTTPException
from playwright.async_api import async_playwright, Browser, BrowserContext, Page
from playwright_stealth import Stealth
from bs4 import BeautifulSoup
import uvicorn

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger("boss-mcp")

MAC_CHROME_146_UA = (
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_5) "
    "AppleWebKit/537.36 (KHTML, like Gecko) "
    "Chrome/146.0.7680.178 Safari/537.36"
)
DEFAULT_FP_LOCALE = os.getenv("BOSS_FP_LOCALE", "en-US")
DEFAULT_FP_TIMEZONE = os.getenv("BOSS_FP_TIMEZONE", "Asia/Shanghai")
DEFAULT_FP_LANG_HEADER = os.getenv("BOSS_FP_ACCEPT_LANGUAGE", "en-US,en;q=0.9,zh-CN;q=0.8,zh;q=0.7")


def _get_env_delay_seconds(name: str, default: float) -> float:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        value = float(raw)
    except ValueError:
        logger.warning(f"Invalid {name}={raw}, fallback to {default}")
        return default
    if value < 0:
        logger.warning(f"Negative {name}={raw}, fallback to {default}")
        return default
    return value


def build_fingerprint_init_script() -> str:
    return """
Object.defineProperty(navigator, 'webdriver', { get: () => undefined });
Object.defineProperty(navigator, 'platform', { get: () => 'MacIntel' });
Object.defineProperty(navigator, 'language', { get: () => 'en-US' });
Object.defineProperty(navigator, 'languages', { get: () => ['en-US', 'en', 'zh-CN', 'zh'] });
Object.defineProperty(navigator, 'hardwareConcurrency', { get: () => 8 });
Object.defineProperty(navigator, 'deviceMemory', { get: () => 8 });
"""


def browser_launch_kwargs() -> Dict[str, Any]:
    kwargs: Dict[str, Any] = {
        "headless": True,
        "args": [
            "--disable-blink-features=AutomationControlled",
            "--window-size=1920,1080",
            "--lang=zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7",
            "--disable-features=IsolateOrigins,site-per-process",
        ],
    }
    proxy = get_browser_proxy()
    if proxy:
        logger.info(f"Playwright browser using proxy: {proxy['server']}")
        kwargs["proxy"] = proxy
    return kwargs


def browser_context_kwargs() -> Dict[str, Any]:
    return {
        "user_agent": MAC_CHROME_146_UA,
        "viewport": {"width": 1920, "height": 1080},
        "device_scale_factor": 2,
        "locale": DEFAULT_FP_LOCALE,
        "timezone_id": DEFAULT_FP_TIMEZONE,
        "color_scheme": "light",
        "extra_http_headers": {
            "Accept-Language": DEFAULT_FP_LANG_HEADER,
            "sec-ch-ua-platform": '"macOS"',
        },
    }


@asynccontextmanager
async def lifespan(app: FastAPI):
    yield
    bm = await get_browser_manager()
    await bm.close()


app = FastAPI(title="BossZhipin MCP Bridge", lifespan=lifespan)


def get_browser_proxy() -> Optional[Dict[str, str]]:
    proxy_server = (
        os.getenv("BOSS_MCP_PROXY")
        or os.getenv("HTTPS_PROXY")
        or os.getenv("https_proxy")
        or os.getenv("HTTP_PROXY")
        or os.getenv("http_proxy")
    )
    if not proxy_server:
        return None
    return {"server": proxy_server}


@app.get("/health")
async def health() -> Dict[str, str]:
    return {"status": "ok"}


class RateLimiter:
    def __init__(self, requests_per_minute: int = 30, burst: int = 10):
        self.requests_per_minute = requests_per_minute
        self.burst = burst
        self._timestamps: List[float] = []
        self._lock = asyncio.Lock()

    async def acquire(self) -> bool:
        async with self._lock:
            now = time.time()
            window_start = now - 60.0
            self._timestamps = [t for t in self._timestamps if t > window_start]
            
            if len(self._timestamps) >= self.burst:
                return False
            
            self._timestamps.append(now)
            return True

    async def wait_and_acquire(self, timeout: float = 30.0) -> bool:
        start = time.time()
        while True:
            if await self.acquire():
                return True
            if time.time() - start > timeout:
                return False
            await asyncio.sleep(1.0)


rate_limiter = RateLimiter(requests_per_minute=30, burst=10)


login_wait_lock = asyncio.Lock()
login_wait_task: Optional[asyncio.Task] = None
cached_login_cookie: Optional[str] = None
cached_login_cookie_at: float = 0.0
qr_flow_lock = asyncio.Lock()
cached_qr_result: Optional[Dict[str, Any]] = None
cached_qr_result_at: float = 0.0
QR_RESULT_CACHE_SECONDS = max(1.0, _get_env_delay_seconds("BOSS_QR_RESULT_CACHE_SECONDS", 10.0))
COOKIE_CACHE_PATH = os.getenv(
    "BOSS_COOKIE_CACHE_PATH",
    os.path.join(os.path.dirname(__file__), "boss_login_cookie.json"),
)


def _ensure_parent_dir(path: str) -> None:
    parent = os.path.dirname(path)
    if parent:
        os.makedirs(parent, exist_ok=True)


def save_cached_login_cookie(cookie: Optional[str]) -> None:
    if not cookie:
        return
    try:
        _ensure_parent_dir(COOKIE_CACHE_PATH)
        payload = {
            "cookie": cookie,
            "saved_at": int(time.time()),
        }
        with open(COOKIE_CACHE_PATH, "w", encoding="utf-8") as f:
            json.dump(payload, f, ensure_ascii=False)
    except Exception as e:
        logger.warning(f"Failed to save cookie cache file: {e}")


def read_cookie_cache_file(max_age_seconds: int = 1800) -> Optional[str]:
    if not os.path.exists(COOKIE_CACHE_PATH):
        return None
    try:
        with open(COOKIE_CACHE_PATH, "r", encoding="utf-8") as f:
            payload = json.load(f)
        cookie = payload.get("cookie")
        saved_at = int(payload.get("saved_at", 0))
        if not cookie or not saved_at:
            return None
        if (time.time() - saved_at) > max_age_seconds:
            return None
        return cookie
    except Exception as e:
        logger.warning(f"Failed to read cookie cache file: {e}")
        return None


def _format_zhipin_cookie_from_cookie_list(cookies: List[Dict[str, Any]]) -> Optional[str]:
    parts = []
    for c in cookies:
        domain = (c.get("domain") or "").lower()
        if "zhipin" not in domain:
            continue
        name = c.get("name")
        value = c.get("value")
        if name and value:
            parts.append(f"{name}={value}")
    if not parts:
        return None
    return "; ".join(parts)


def _http_post_json(url: str, payload: Dict[str, Any], headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
    raw = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib_request.Request(url=url, data=raw, method="POST")
    req.add_header("Content-Type", "application/json; charset=utf-8")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    with urllib_request.urlopen(req, timeout=20) as resp:
        body = resp.read().decode("utf-8", errors="replace")
        if not body:
            return {}
        return json.loads(body)


def _http_post_multipart(url: str, fields: Dict[str, str], files: Dict[str, Dict[str, Any]], headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
    boundary = f"----bossmcp{uuid.uuid4().hex}"
    body = bytearray()

    for name, value in fields.items():
        body.extend(f"--{boundary}\r\n".encode("utf-8"))
        body.extend(f"Content-Disposition: form-data; name=\"{name}\"\r\n\r\n".encode("utf-8"))
        body.extend(value.encode("utf-8"))
        body.extend(b"\r\n")

    for name, info in files.items():
        filename = info.get("filename", "upload.bin")
        content_type = info.get("content_type", "application/octet-stream")
        data = info.get("data", b"")
        body.extend(f"--{boundary}\r\n".encode("utf-8"))
        body.extend(
            f"Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n".encode("utf-8")
        )
        body.extend(f"Content-Type: {content_type}\r\n\r\n".encode("utf-8"))
        body.extend(data)
        body.extend(b"\r\n")

    body.extend(f"--{boundary}--\r\n".encode("utf-8"))

    req = urllib_request.Request(url=url, data=bytes(body), method="POST")
    req.add_header("Content-Type", f"multipart/form-data; boundary={boundary}")
    for k, v in (headers or {}).items():
        req.add_header(k, v)

    with urllib_request.urlopen(req, timeout=30) as resp:
        raw = resp.read().decode("utf-8", errors="replace")
        if not raw:
            return {}
        return json.loads(raw)


def _send_qr_to_feishu_sync(qr_base64: str, image_type: str = "png") -> Dict[str, Any]:
    webhook_url = os.getenv("BOSS_FEISHU_WEBHOOK_URL", "").strip()
    app_id = os.getenv("BOSS_FEISHU_APP_ID", "").strip()
    app_secret = os.getenv("BOSS_FEISHU_APP_SECRET", "").strip()
    chat_id = os.getenv("BOSS_FEISHU_CHAT_ID", "").strip()

    if not qr_base64:
        return {"sent": False, "reason": "empty_qr"}

    # Preferred path: use official open-api (token -> upload image -> send image message).
    if app_id and app_secret and chat_id:
        image_bytes = base64.b64decode(qr_base64)
        auth_resp = _http_post_json(
            "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal",
            {"app_id": app_id, "app_secret": app_secret},
        )
        if auth_resp.get("code") != 0:
            return {"sent": False, "reason": "auth_failed", "detail": auth_resp}

        token = auth_resp.get("tenant_access_token", "")
        if not token:
            return {"sent": False, "reason": "missing_token", "detail": auth_resp}

        upload_resp = _http_post_multipart(
            "https://open.feishu.cn/open-apis/im/v1/images",
            fields={"image_type": "message"},
            files={
                "image": {
                    "filename": f"boss_qr.{image_type or 'png'}",
                    "content_type": f"image/{image_type or 'png'}",
                    "data": image_bytes,
                }
            },
            headers={"Authorization": f"Bearer {token}"},
        )
        if upload_resp.get("code") != 0:
            return {"sent": False, "reason": "upload_failed", "detail": upload_resp}

        image_key = ((upload_resp.get("data") or {}).get("image_key") or "").strip()
        if not image_key:
            return {"sent": False, "reason": "missing_image_key", "detail": upload_resp}

        send_resp = _http_post_json(
            "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=chat_id",
            {
                "receive_id": chat_id,
                "msg_type": "image",
                "content": json.dumps({"image_key": image_key}, ensure_ascii=False),
            },
            headers={"Authorization": f"Bearer {token}"},
        )
        if send_resp.get("code") != 0:
            return {"sent": False, "reason": "send_failed", "detail": send_resp}

        return {"sent": True, "mode": "open_api_image", "image_key": image_key}

    # Fallback path: webhook text notification (without image upload capability).
    if webhook_url:
        digest = hashlib.sha256(qr_base64.encode("utf-8")).hexdigest()[:16]
        payload = {
            "msg_type": "text",
            "content": {
                "text": (
                    "Boss二维码已生成。"
                    f" image_type={image_type or 'png'}, base64_len={len(qr_base64)}, sha256_16={digest}. "
                    "请在系统界面中查看二维码并扫码登录。"
                )
            },
        }
        _http_post_json(webhook_url, payload)
        return {"sent": True, "mode": "webhook_text"}

    return {"sent": False, "reason": "feishu_not_configured"}


async def send_qr_to_feishu(qr_base64: str, image_type: str = "png") -> Dict[str, Any]:
    try:
        result = await asyncio.to_thread(_send_qr_to_feishu_sync, qr_base64, image_type)
        if result.get("sent"):
            logger.info(f"Feishu notification sent, mode={result.get('mode', '')}")
        else:
            logger.info(f"Feishu notification skipped/failed: {result}")
        return result
    except Exception as e:
        logger.warning(f"Send QR to Feishu failed: {e}")
        return {"sent": False, "reason": "exception", "detail": str(e)}


def cache_login_cookie(cookie: Optional[str]) -> None:
    global cached_login_cookie, cached_login_cookie_at
    if cookie:
        cached_login_cookie = cookie
        cached_login_cookie_at = time.time()
        save_cached_login_cookie(cookie)


def read_cached_login_cookie(max_age_seconds: int = 1800) -> Optional[str]:
    if cached_login_cookie and (time.time() - cached_login_cookie_at) <= max_age_seconds:
        return cached_login_cookie
    file_cookie = read_cookie_cache_file(max_age_seconds=max_age_seconds)
    if file_cookie:
        cache_login_cookie(file_cookie)
        return file_cookie
    return None


def _clone_json_dict(data: Dict[str, Any]) -> Dict[str, Any]:
    # Keep cache payload isolated from per-request mutations.
    return json.loads(json.dumps(data, ensure_ascii=False))


def read_cached_qr_result(max_age_seconds: Optional[float] = None) -> Optional[Dict[str, Any]]:
    if cached_qr_result is None:
        return None
    ttl = QR_RESULT_CACHE_SECONDS if max_age_seconds is None else max(0.1, float(max_age_seconds))
    if (time.time() - cached_qr_result_at) > ttl:
        return None
    result = _clone_json_dict(cached_qr_result)
    result["qr_cache_hit"] = True
    result["qr_cache_ttl_seconds"] = ttl
    return result


def write_cached_qr_result(result: Dict[str, Any]) -> None:
    global cached_qr_result, cached_qr_result_at
    cacheable = bool(result.get("qrcode_base64")) or bool(result.get("is_login"))
    if not cacheable:
        cached_qr_result = None
        cached_qr_result_at = 0.0
        return
    clean = {
        "is_login": bool(result.get("is_login", False)),
        "qrcode_base64": result.get("qrcode_base64", ""),
        "image_type": result.get("image_type", "png"),
        "description": result.get("description", ""),
        "message": result.get("message", "QR code captured successfully"),
    }
    cached_qr_result = _clone_json_dict(clean)
    cached_qr_result_at = time.time()


async def acquire_qr_result(context: BrowserContext, force_refresh: bool = False) -> Dict[str, Any]:
    if not force_refresh:
        cached = read_cached_qr_result()
        if cached is not None:
            logger.info("QR result cache hit (without lock)")
            return cached

    async with qr_flow_lock:
        if not force_refresh:
            cached = read_cached_qr_result()
            if cached is not None:
                logger.info("QR result cache hit (with lock)")
                return cached

        result = await get_qrcode_base64(context)
        write_cached_qr_result(result)
        result = _clone_json_dict(result)
        result["qr_cache_hit"] = False
        result["qr_cache_ttl_seconds"] = QR_RESULT_CACHE_SECONDS
        return result


class BrowserManager:
    _instance: Optional['BrowserManager'] = None
    _lock = asyncio.Lock()

    def __init__(self):
        self.pw = None
        self.browser: Optional[Browser] = None
        self.login_context: Optional[BrowserContext] = None
        self.cookie_context: Optional[BrowserContext] = None
        # 短信登录两步法之间保活的登录页 (login_send_sms -> login_submit_sms)
        self.login_page: Optional[Page] = None
        self.login_page_at: float = 0.0

    @classmethod
    async def get_instance(cls) -> 'BrowserManager':
        async with cls._lock:
            if cls._instance is None:
                cls._instance = cls()
            return cls._instance

    async def get_login_context(self) -> BrowserContext:
        async with self._lock:
            if self.login_context is None:
                if self.browser is None:
                    if self.pw is None:
                        self.pw = await async_playwright().start()
                    self.browser = await self.pw.chromium.launch(**browser_launch_kwargs())
                self.login_context = await self.browser.new_context(**browser_context_kwargs())
                await self.login_context.add_init_script(build_fingerprint_init_script())
                page = await self.login_context.new_page()
                await Stealth().apply_stealth_async(page)
                await page.goto("https://www.zhipin.com", wait_until="networkidle")
                await page.close()
            return self.login_context

    async def get_persistent_login_page(self, force_new: bool = False) -> Page:
        """返回一个在两次 HTTP 调用间保活的登录页 (用于短信验证码两步登录)。
        与 QR 流程不同, 这里不会 page.close(), 以便 login_send_sms 发码后,
        login_submit_sms 能在同一页面填入验证码完成登录。
        过期 (BOSS_LOGIN_PAGE_TTL, 默认 300s) 或 force_new 时重建。"""
        ttl = _get_env_delay_seconds("BOSS_LOGIN_PAGE_TTL", 300.0)
        # 复用 / 初始化 login_context (内部已持 self._lock)
        context = await self.get_login_context()
        async with self._lock:
            page = self.login_page
            expired = (time.time() - self.login_page_at) > ttl
            stale = False
            if page is not None:
                try:
                    stale = page.is_closed()
                except Exception:
                    stale = True
            if force_new or expired or stale or page is None:
                if page is not None:
                    try:
                        await page.close()
                    except Exception:
                        pass
                page = await context.new_page()
                await Stealth().apply_stealth_async(page)
                self.login_page = page
                self.login_page_at = time.time()
            return page

    async def get_cookie_context(self, cookie_str: str) -> BrowserContext:
        async with self._lock:
            cookies = []
            for item in cookie_str.split(';'):
                item = item.strip()
                if not item or '=' not in item:
                    continue
                idx = item.index('=')
                name = item[:idx].strip()
                value = item[idx+1:].strip()
                if name and value:
                    cookies.append({
                        "name": name,
                        "value": value,
                        "domain": ".zhipin.com",
                        "path": "/"
                    })

            if self.cookie_context:
                await self.cookie_context.close()

            if self.browser is None:
                if self.pw is None:
                    self.pw = await async_playwright().start()
                self.browser = await self.pw.chromium.launch(**browser_launch_kwargs())

            self.cookie_context = await self.browser.new_context(**browser_context_kwargs())
            await self.cookie_context.add_init_script(build_fingerprint_init_script())
            await self.cookie_context.add_cookies(cookies)
            return self.cookie_context

    async def transfer_login_cookies(self) -> Optional[str]:
        async with self._lock:
            if self.login_context is None:
                return None
            try:
                cookies = await self.login_context.cookies()
                cookie_str = "; ".join([f"{c['name']}={c['value']}" for c in cookies if 'zhipin' in c.get('domain', '')])
                return cookie_str if cookie_str else None
            except Exception as e:
                logger.error(f"Failed to transfer cookies: {e}")
                return None

    async def reset_login_context(self) -> None:
        """Close and discard the current login_context so the next
        get_login_context() call will create a fresh one.  This is
        essential to recover from stale / blocked browser state that
        causes persistent QR capture failures (e.g. about:blank)."""
        async with self._lock:
            if self.login_page is not None:
                try:
                    await self.login_page.close()
                except Exception:
                    pass
                self.login_page = None
                self.login_page_at = 0.0
            if self.login_context is not None:
                try:
                    await self.login_context.close()
                except Exception as exc:
                    logger.warning("Failed to close stale login_context: %s", exc)
                self.login_context = None
            # Also clear cookie_context – it may have been derived from
            # cookies that are no longer valid after the login context is
            # reset.
            if self.cookie_context is not None:
                try:
                    await self.cookie_context.close()
                except Exception as exc:
                    logger.warning("Failed to close cookie_context on reset: %s", exc)
                self.cookie_context = None
            logger.info("login_context and cookie_context reset – next call will create fresh contexts")

    async def close(self):
        async with self._lock:
            if self.login_page:
                try:
                    await self.login_page.close()
                except Exception:
                    pass
                self.login_page = None
                self.login_page_at = 0.0
            if self.login_context:
                await self.login_context.close()
                self.login_context = None
            if self.cookie_context:
                await self.cookie_context.close()
                self.cookie_context = None
            if self.browser:
                await self.browser.close()
                self.browser = None
            if self.pw:
                await self.pw.stop()
                self.pw = None


browser_manager: Optional[BrowserManager] = None


async def get_browser_manager() -> BrowserManager:
    global browser_manager
    if browser_manager is None:
        browser_manager = await BrowserManager.get_instance()
    return browser_manager


class BossScraper:
    def __init__(self, context: BrowserContext):
        self.context = context

    async def _auth_required_payload(self, message: str) -> Dict[str, Any]:
        bm = await get_browser_manager()
        login_ctx = await bm.get_login_context()
        qr_result = await acquire_qr_result(login_ctx)
        if qr_result.get("is_login"):
            await start_background_wait_for_login(timeout=240)
            return {
                "success": False,
                "status": "auth_required",
                "message": "检测到浏览器已登录，请调用 boss.wait_login 获取并保存 cookie",
            }

        should_start_wait = bool(qr_result.get("qrcode_base64"))
        if not should_start_wait and not qr_result.get("is_login"):
            # QR capture returned empty — reset stale context so next attempt gets fresh one
            logger.warning("QR capture returned empty base64, resetting login context for next attempt")
            await bm.reset_login_context()
        if should_start_wait and not qr_result.get("qr_cache_hit"):
            feishu_result = await send_qr_to_feishu(
                qr_result.get("qrcode_base64", ""),
                qr_result.get("image_type", "png"),
            )
        if should_start_wait:
            await start_background_wait_for_login(timeout=240)

        return {
            "success": False,
            "status": "auth_required",
            "message": message,
            "qrcode_image": {
                "image_data": qr_result.get("qrcode_base64", ""),
                "image_type": qr_result.get("image_type", "png"),
                "description": qr_result.get("description", "请使用Boss直聘APP扫描此二维码登录"),
            },
            "next_step": "",
        }

    async def search_jobs(self, query: str, city: str = "", limit: int = 10) -> Any:
        page = await self.context.new_page()
        await Stealth().apply_stealth_async(page)

        city_codes = {
            "广州": "101280100",
            "北京": "101010100",
            "上海": "101020100",
            "深圳": "101280600",
            "杭州": "101210100",
            "成都": "101270100",
            "武汉": "101200100",
            "南京": "101190100",
            "西安": "101110100",
            "重庆": "101040100",
            "天津": "101030100",
            "苏州": "101190400",
            "郑州": "101180100",
            "长沙": "101250100",
            "东莞": "101281600",
            "沈阳": "101070100",
            "青岛": "101120200",
            "合肥": "101220100",
            "佛山": "101280800",
            "济南": "101120100",
            "昆明": "101290100",
            "福州": "101230100",
            "哈尔滨": "101050100",
            "大连": "101070200",
            "长春": "101060100",
            "石家庄": "101090100",
            "厦门": "101230200",
            "无锡": "101190200",
            "宁波": "101210400",
            "南昌": "101240100",
        }
        code = city_codes.get(city, "101280100")

        encoded_query = quote(query, safe="")
        url = f"https://www.zhipin.com/web/geek/job?query={encoded_query}&city={code}"

        logger.info(f"Searching: {url}")
        await page.goto(url, wait_until="networkidle")
        await page.wait_for_load_state("domcontentloaded")

        card_selector = ".job-card-wrapper, .job-card-box"
        empty_selector = ".job-empty, .search-empty"
        login_selector = ".login-btn, .user-nav, .geetest, [class*='captcha']"

        try:
            await page.wait_for_selector(f"{card_selector}, {empty_selector}", timeout=15000)
        except Exception:
            current_url = page.url
            content = await page.content()
            soup = BeautifulSoup(content, 'html.parser')
            page_text = soup.get_text(" ", strip=True)
            page_title = await page.title()

            # Heuristic anti-bot / verification detection.
            anti_bot_keywords = [
                "验证码", "滑块", "验证", "风控", "访问异常", "安全验证", "verify", "captcha"
            ]
            is_antibot = any(k.lower() in page_text.lower() for k in anti_bot_keywords)
            is_blank_or_tiny = len((content or "").strip()) < 500

            if "login" in current_url or soup.select_one(login_selector) or is_antibot or is_blank_or_tiny:
                await page.close()
                msg = "Boss cookie 失效或触发验证，请扫码登录"
                if is_blank_or_tiny:
                    msg = "Boss 页面返回空白/受限内容，疑似触发风控，请扫码登录"
                return await self._auth_required_payload(msg)

            # Selector changed but page may still have job links; try structural fallback.
            fallback_links = []
            for a in soup.select("a[href*='job_detail']"):
                href = (a.get("href") or "").strip()
                if href and href not in fallback_links:
                    fallback_links.append(href)
                if len(fallback_links) >= limit:
                    break

            if fallback_links:
                job_list = []
                for href in fallback_links[:limit]:
                    job_id = href.split('/')[-1].split('.')[0] if '/' in href else href
                    title = "N/A"
                    try:
                        title = href.split("/")[-1].split(".")[0]
                    except Exception:
                        pass
                    job_list.append({
                        "job_id": job_id,
                        "job_title": title,
                        "salary": "N/A",
                        "company": "N/A",
                        "city": city or "N/A",
                        "experience": ""
                    })
                await page.close()
                return job_list

            debug_excerpt = page_text[:240]
            await page.close()
            raise Exception(
                f"Job list selector not found; page may have changed or blocked. "
                f"title={page_title}, url={current_url}, html_len={len(content)}, excerpt={debug_excerpt}"
            )

        content = await page.content()
        soup = BeautifulSoup(content, 'html.parser')
        job_list = []

        if soup.select_one(empty_selector):
            await page.close()
            return []

        items = soup.select(".job-card-wrapper, .job-card-box")
        for item in items[:limit]:
            try:
                link = item.select_one("a.job-card-left") or item.select_one("a[href*='job_detail']")
                if not link:
                    continue

                href = link.get('href', '')
                if not href:
                    continue
                job_id = href.split('/')[-1].split('.')[0]

                title_node = item.select_one(".job-name") or item.select_one(".job-title")
                salary_node = item.select_one(".salary") or item.select_one(".job-salary")
                company_node = item.select_one(".company-name") or item.select_one(".company-text")
                city_node = item.select_one(".job-area") or item.select_one(".job-area-wrapper")
                exp_node = item.select_one(".tag-list") or item.select_one(".job-info")

                job_list.append({
                    "job_id": job_id,
                    "job_title": title_node.text.strip() if title_node else "N/A",
                    "salary": salary_node.text.strip() if salary_node else "N/A",
                    "company": company_node.text.strip() if company_node else "N/A",
                    "city": city_node.text.strip() if city_node else "N/A",
                    "experience": exp_node.text.strip() if exp_node else ""
                })
            except Exception as e:
                logger.error(f"Error parsing job item: {e}")

        await page.close()
        return job_list

    async def get_job_detail(self, job_id: str) -> Dict:
        page = await self.context.new_page()
        await Stealth().apply_stealth_async(page)
        url = f"https://www.zhipin.com/job_detail/{job_id}.html"

        logger.info(f"Fetching JD: {url}")
        await page.goto(url, wait_until="networkidle")
        await page.wait_for_selector(".job-sec-text", timeout=10000)

        content = await page.content()
        soup = BeautifulSoup(content, 'html.parser')

        detail = {
            "job_id": job_id,
            "description": soup.select_one(".job-sec-text").text.strip() if soup.select_one(".job-sec-text") else "N/A",
            "company_info": soup.select_one(".company-info").text.strip() if soup.select_one(".company-info") else "N/A"
        }
        await page.close()
        return detail

    async def check_auth(self) -> bool:
        # Cookie-only auth check: avoid relying on page redirects/selectors.
        try:
            cookies = await self.context.cookies("https://www.zhipin.com")
        except Exception as e:
            logger.warning(f"check_auth cookies read failed: {e}")
            return False

        now = time.time()
        has_auth_cookie = False
        valid_zhipin_cookie_count = 0

        for c in cookies:
            domain = (c.get("domain") or "").lower()
            if "zhipin" not in domain:
                continue

            expires = c.get("expires")
            if isinstance(expires, (int, float)) and expires > 0 and expires < now:
                continue

            name = (c.get("name") or "").strip().lower()
            if not name:
                continue

            valid_zhipin_cookie_count += 1
            if (
                name in {"wt2", "__zp_stoken__", "zp_token", "stoken", "token", "t"}
                or "token" in name
                or "stoken" in name
                or name.startswith("wt")
            ):
                has_auth_cookie = True

        if has_auth_cookie:
            return True

        logger.info(
            f"Auth check failed by cookie inspection: valid_zhipin_cookie_count={valid_zhipin_cookie_count}"
        )
        return False

    async def apply_job(self, job_id: str) -> Dict[str, Any]:
        page = await self.context.new_page()
        await Stealth().apply_stealth_async(page)
        
        url = f"https://www.zhipin.com/job_detail/{job_id}.html"
        logger.info(f"Applying to job: {url}")
        
        try:
            await page.goto(url, wait_until="networkidle")
            await page.wait_for_selector(".job-sec-text", timeout=10000)
            
            # Try multiple possible selectors for the apply button
            apply_selectors = [
                ".btn-apply",
                ".btn-chat",
                "a.btn-startchat",
                ".op-btn.apply-btn",
                ".job-btn.btn-apply"
            ]

            applied = False
            for selector in apply_selectors:
                try:
                    button = await page.query_selector(selector)
                    if button:
                        await button.click()
                        try:
                            await page.wait_for_selector(".dialog-content, .success-tip, .apply-success", timeout=3000)
                        except:
                            pass
                        applied = True
                        logger.info(f"Clicked apply button: {selector}")
                        break
                except Exception as e:
                    logger.debug(f"Selector {selector} not found: {e}")

            if not applied:
                await page.close()
                return {"success": False, "message": "未找到投递按钮，可能已投递或职位已下线"}
            
            await page.close()
            return {"success": True, "message": "投递成功"}
            
        except Exception as e:
            logger.error(f"Apply job error: {e}")
            await page.close()
            return {"success": False, "message": f"投递失败: {str(e)}"}


async def get_qrcode_base64(context: BrowserContext) -> Dict[str, Any]:
    page = await context.new_page()
    await Stealth().apply_stealth_async(page)
    started_at = time.monotonic()

    login_state_selectors = [
        ".user-info",
        ".nav-figure",
        ".geek-name",
        ".user-name",
        ".nick-name",
        ".user-nav",
    ]
    qrcode_selectors = [
        ".qr-img-box img",
        ".qr-img-box canvas",
        ".qr-img-box",
        ".qrcode-wrapper img",
        ".qrcode-wrapper canvas",
        ".login-scan-wrap img",
        ".login-scan-wrap canvas",
        "img.qrcode",
        "canvas.qrcode",
        "[class*='qrcode'] img",
        "[class*='qrcode'] canvas",
        "[class*='qr'] img",
        "[class*='qr'] canvas",
        ".scan-qrcode img",
        "#qrcode img",
        ".login-qrcode img",
    ]
    qr_tab_selectors = [
        "text=APP扫码登录",
        "text=扫码登录",
        ".login-way-tabs >> text=APP扫码登录",
        ".login-type-switch >> text=APP扫码登录",
        ".scan-login-tab",
        "a[tab='scan']",
        "[data-tab='scan']",
        "a:has-text('APP扫码登录')",
        "a:has-text('扫码登录')"
    ]

    async def recreate_page(reason: str) -> None:
        nonlocal page
        try:
            await page.close()
        except Exception:
            pass
        page = await context.new_page()
        await Stealth().apply_stealth_async(page)
        logger.warning(f"Recreated Playwright page, reason={reason}")

    async def open_login_surface() -> bool:
        home_url = "https://www.zhipin.com"
        login_url = "https://www.zhipin.com/web/user/?ka=header-login"

        async def _try_nav(url: str, wait_until: str) -> None:
            try:
                resp = await page.goto(url, wait_until=wait_until, timeout=45000)
                status = resp.status if resp is not None else None
                logger.info("goto url=%s wait=%s status=%s final_url=%s", url, wait_until, status, page.url)
                await page.wait_for_timeout(1800)
            except Exception as e:
                logger.warning(f"Navigation failed target={url}, err={e}")

        await _try_nav(home_url, "domcontentloaded")
        await _try_nav(login_url, "domcontentloaded")

        if (page.url or "").startswith("about:blank"):
            await _try_nav(login_url, "load")
            
        if not (page.url or "").startswith("about:blank"):
            return True
        return False

    async def all_scopes() -> List[Any]:
        scopes: List[Any] = [page]
        for frame in page.frames:
            if frame != page.main_frame:
                scopes.append(frame)
        return scopes

    async def find_first(selectors: List[str]) -> Optional[Any]:
        for scope in await all_scopes():
            for selector in selectors:
                try:
                    el = await scope.query_selector(selector)
                    if el is not None:
                        return el
                except Exception:
                    continue
        return None

    async def switch_to_scan_mode() -> None:
        for selector in qr_tab_selectors:
            try:
                tab = page.locator(selector).first
                if await tab.count() > 0:
                    await tab.click(force=True)
                    logger.info("Clicked QR login tab with selector: %s", selector)
                    await asyncio.sleep(0.5)
                    return
            except Exception as e:
                logger.debug("QR login tab selector %s not found or click failed: %s", selector, e)
        logger.warning("Could not find or click any QR login tab selector.")

    async def fetch_image_as_base64(src: str) -> Optional[str]:
        if not src:
            return None
        if src.startswith("data:image") and "," in src:
            return src.split(",", 1)[1]
        if src.startswith("blob:"):
            # Blob URL cannot be fetched by APIRequestContext over HTTP.
            return None

        final_src = src
        if src.startswith("//"):
            final_src = f"https:{src}"
        elif src.startswith("/"):
            final_src = f"https://www.zhipin.com{src}"
        elif not src.startswith("http://") and not src.startswith("https://"):
            base_url = page.url or "https://www.zhipin.com/user/login.html"
            final_src = urljoin(base_url, src)

        try:
            resp = await page.request.get(
                final_src,
                headers={"Referer": "https://www.zhipin.com/user/login.html"},
            )
            if resp.ok:
                body = await resp.body()
                return base64.b64encode(body).decode("utf-8")
            logger.warning(f"Failed to fetch QR image, status={resp.status}, url={final_src}")
        except Exception as e:
            logger.warning(f"Fetch QR image failed, url={final_src}, err={e}")
        return None

    async def extract_qr_payload(el: Any) -> Optional[str]:
        if el is None:
            return None
        try:
            tag_name = await el.evaluate("node => node.tagName.toLowerCase()")
        except Exception:
            return None

        if tag_name == "img":
            try:
                src = await el.get_attribute("src")
                if src:
                    b64 = await fetch_image_as_base64(src)
                    if b64:
                        return b64
            except Exception:
                pass

        if tag_name == "canvas":
            try:
                data_url = await el.evaluate("node => node.toDataURL('image/png')")
                if isinstance(data_url, str) and data_url.startswith("data:image") and "," in data_url:
                    return data_url.split(",", 1)[1]
            except Exception:
                pass

        try:
            child_img = await el.query_selector("img")
            if child_img is not None:
                b64 = await extract_qr_payload(child_img)
                if b64:
                    return b64
            child_canvas = await el.query_selector("canvas")
            if child_canvas is not None:
                b64 = await extract_qr_payload(child_canvas)
                if b64:
                    return b64
        except Exception:
            pass

        return None

    try:
        # Keep defaults consistent with local validated test profile.
        qr_init_delay = _get_env_delay_seconds("BOSS_QR_INIT_DELAY", 0.2)
        qr_probe_window = _get_env_delay_seconds("BOSS_QR_PROBE_WINDOW", 10.0)
        qr_probe_interval = _get_env_delay_seconds("BOSS_QR_PROBE_INTERVAL", 0.25)
        qr_attempts = int(os.getenv("BOSS_QR_ATTEMPTS", "4"))
        if qr_attempts < 1:
            qr_attempts = 1
        if qr_attempts > 5:
            qr_attempts = 5

        fallback_base64 = ""
        fallback_source = ""
        last_url = ""

        for attempt in range(1, qr_attempts + 1):
            if attempt > 1:
                await recreate_page(f"attempt-{attempt}-restart")

            if not await open_login_surface():
                logger.warning(f"Attempt {attempt}: failed to open login surface")
                continue

            this_delay = qr_init_delay if attempt == 1 else min(qr_init_delay, 0.8)
            if this_delay > 0:
                logger.info(
                    f"Attempt {attempt}/{qr_attempts}: wait {this_delay:.2f}s for login panel render"
                )
                await asyncio.sleep(this_delay)

            if (page.url or "").startswith("about:blank"):
                logger.warning(f"Attempt {attempt}: page became about:blank after initial wait")
                continue

            last_url = page.url or ""

            if await find_first(login_state_selectors):
                return {"is_login": True, "message": "Already logged in"}

            await switch_to_scan_mode()

            deadline = time.monotonic() + qr_probe_window
            while time.monotonic() < deadline:
                if (page.url or "").startswith("about:blank"):
                    logger.warning(f"Attempt {attempt}: page became about:blank during probe")
                    break

                last_url = page.url or last_url

                if await find_first(login_state_selectors):
                    return {"is_login": True, "message": "Already logged in"}

                qrcode_element = await find_first(qrcode_selectors)
                if qrcode_element is not None:
                    extracted = await extract_qr_payload(qrcode_element)
                    if extracted and len(extracted) > 2000:
                        elapsed_ms = int((time.monotonic() - started_at) * 1000)
                        logger.info(
                            f"QR captured source=dom_extract, attempt={attempt}, base64_len={len(extracted)}, elapsed_ms={elapsed_ms}, url={page.url}"
                        )
                        return {
                            "is_login": False,
                            "qrcode_base64": extracted,
                            "image_type": "png",
                            "description": "",
                            "message": "QR code captured successfully",
                        }

                    try:
                        screenshot_bytes = await qrcode_element.screenshot()
                        screenshot_base64 = base64.b64encode(screenshot_bytes).decode("utf-8")
                        if len(screenshot_base64) > 2000:
                            elapsed_ms = int((time.monotonic() - started_at) * 1000)
                            logger.info(
                                f"QR captured source=element_screenshot, attempt={attempt}, base64_len={len(screenshot_base64)}, elapsed_ms={elapsed_ms}, url={page.url}"
                            )
                            return {
                                "is_login": False,
                                "qrcode_base64": screenshot_base64,
                                "image_type": "png",
                                "description": "",
                                "message": "QR code captured successfully",
                            }
                    except Exception:
                        pass

                # Keep a non-blank visual fallback from login container.
                try:
                    login_area = await find_first([
                        ".qr-img-box",
                        ".qrcode-wrapper",
                        ".login-scan-wrap",
                        ".login-content",
                    ])
                    if login_area is not None and not fallback_base64:
                        fb_bytes = await login_area.screenshot()
                        fb_b64 = base64.b64encode(fb_bytes).decode("utf-8")
                        if len(fb_b64) > 2000:
                            fallback_base64 = fb_b64
                            fallback_source = "container_screenshot"
                except Exception:
                    pass

                await asyncio.sleep(max(0.1, qr_probe_interval))

        # Final fallback: use best non-blank capture if available.
        if fallback_base64:
            elapsed_ms = int((time.monotonic() - started_at) * 1000)
            logger.info(
                f"QR fallback captured source={fallback_source}, base64_len={len(fallback_base64)}, elapsed_ms={elapsed_ms}, url={last_url}"
            )
            return {
                "is_login": False,
                "qrcode_base64": fallback_base64,
                "image_type": "png",
                "description": "",
                "message": "QR capture fallback image returned",
            }

        # Avoid returning a guaranteed blank screenshot when url is already about:blank.
        if not (page.url or "").startswith("about:blank"):
            screenshot_bytes = await page.screenshot(full_page=True)
            screenshot_base64 = base64.b64encode(screenshot_bytes).decode("utf-8")
            elapsed_ms = int((time.monotonic() - started_at) * 1000)
            logger.info(
                f"QR captured source=full_page_screenshot, base64_len={len(screenshot_base64)}, elapsed_ms={elapsed_ms}, url={page.url}"
            )
            return {
                "is_login": False,
                "qrcode_base64": screenshot_base64,
                "image_type": "png",
                "description": "",
                "message": "QR code captured with full-page fallback",
            }

        elapsed_ms = int((time.monotonic() - started_at) * 1000)
        logger.warning(
            f"QR capture failed after {qr_attempts} attempts: final_url={page.url}, last_url={last_url}, elapsed_ms={elapsed_ms}"
        )
        try:
            bm = await get_browser_manager()
            await bm.reset_login_context()
            logger.info("Auto-reset login_context after persistent QR capture failure")
        except Exception as reset_exc:
            logger.warning("Failed to auto-reset login_context: %s", reset_exc)
        return {
            "is_login": False,
            "qrcode_base64": "",
            "image_type": "png",
            "description": "",
            "message": "二维码获取失败：登录页持续空白，请检查代理或风控状态后重试",
        }
    finally:
        try:
            await page.close()
        except Exception:
            pass


async def wait_for_login(context: BrowserContext, timeout: int = 120) -> Dict[str, Any]:
    cached = read_cached_login_cookie()
    if cached:
        return {
            "success": True,
            "message": "Login already captured from cached session",
            "cookie": cached,
            "from_cache": True,
        }

    bm = await get_browser_manager()
    HOME_URL = "https://www.zhipin.com"
    login_url = "https://www.zhipin.com/web/user/?ka=header-login"

    async def _try_collect_cookie_from_context(ctx: BrowserContext) -> Optional[str]:
        try:
            cookies = await ctx.cookies()
            cookie_str = _format_zhipin_cookie_from_cookie_list(cookies)
            if cookie_str:
                return cookie_str
        except Exception:
            pass
        return await bm.transfer_login_cookies()

    start_time = asyncio.get_event_loop().time()

    while True:
        elapsed = asyncio.get_event_loop().time() - start_time
        if elapsed > timeout:
            return {"success": False, "message": f"Login timeout after {timeout} seconds"}

        cookie_str = await _try_collect_cookie_from_context(context)
        if cookie_str:
            cache_login_cookie(cookie_str)
            return {
                "success": True,
                "message": "Login successful (cookie detected)",
                "cookie": cookie_str,
                "cookie_saved": True,
            }

        page = None
        try:
            page = await context.new_page()
            await Stealth().apply_stealth_async(page)

            async def _try_nav(url: str, wait_until: str) -> None:
                try:
                    resp = await page.goto(url, wait_until=wait_until, timeout=45000)
                    status = resp.status if resp is not None else None
                    logger.info("wait_for_login goto url=%s wait=%s status=%s final_url=%s", url, wait_until, status, page.url)
                    await page.wait_for_timeout(1800)
                except Exception as e:
                    logger.warning("wait_for_login navigation failed target=%s, err=%s", url, e)

            await _try_nav(HOME_URL, "domcontentloaded")

            current_url = page.url or ""
            if "/web/geek/" in current_url and "login" not in current_url:
                cookie_str = await _try_collect_cookie_from_context(context)
                if cookie_str:
                    cache_login_cookie(cookie_str)
                    await page.close()
                    return {
                        "success": True,
                        "message": "Login successful (redirect detected)",
                        "cookie": cookie_str,
                        "cookie_saved": True,
                    }

            login_selectors = [".user-info", ".nav-figure", ".geek-name", ".user-nav"]
            for sel in login_selectors:
                try:
                    el = await page.query_selector(sel)
                    if el is not None:
                        cookie_str = await _try_collect_cookie_from_context(context)
                        if cookie_str:
                            cache_login_cookie(cookie_str)
                            await page.close()
                            return {
                                "success": True,
                                "message": "Login successful (UI marker detected)",
                                "cookie": cookie_str,
                                "cookie_saved": True,
                            }
                        logger.warning("Login marker found but cookie extraction empty, keep waiting")
                        break
                except Exception:
                    continue

            scan_keywords = ["扫码成功", "请在手机上确认", "请在手机端确认", "确认登录", "已扫码"]
            try:
                page_text = await page.evaluate("() => document.body ? document.body.innerText : ''")
                if isinstance(page_text, str):
                    lower_text = page_text.lower()
                    for kw in scan_keywords:
                        if kw.lower() in lower_text:
                            logger.info("Scan completed, waiting for mobile confirmation")
                            break
            except Exception:
                pass

            cookie_str = await _try_collect_cookie_from_context(context)
            if cookie_str:
                cache_login_cookie(cookie_str)
                await page.close()
                return {
                    "success": True,
                    "message": "Login successful (cookie appeared after scan)",
                    "cookie": cookie_str,
                    "cookie_saved": True,
                }

        except Exception as e:
            logger.warning("wait_for_login page probe error: %s", e)
        finally:
            if page is not None:
                try:
                    await page.close()
                except Exception:
                    pass

        await asyncio.sleep(2.0)


# ---------------------------------------------------------------------------
# 手机号 + 短信验证码登录 (两步法, Agent 对话编排)
# ---------------------------------------------------------------------------

LOGIN_URL_WEB = "https://www.zhipin.com/web/user/?ka=header-login"
LOGIN_URL_OLD = "https://www.zhipin.com/user/login.html"

# best-effort 多选择器: BOSS 登录页 DOM 可能随版本变化, 顺序探测取第一个可见命中
SMS_TAB_SELECTORS = [
    "a[tab='sms']", "a[ka='login-by-sms']",
    "a:has-text('短信登录')", "a:has-text('验证码登录')", "a:has-text('手机登录')",
    "text=短信登录", "text=验证码登录",
]
PHONE_INPUT_SELECTORS = [
    "input[name='phone']", "input[type='tel']",
    "input[placeholder*='手机号']", "input[placeholder*='手机']",
    ".phone-login input[type='text']", "#phone",
]
SEND_CODE_SELECTORS = [
    "button:has-text('获取验证码')", "a:has-text('获取验证码')",
    "button:has-text('发送验证码')", ".btn-sms", ".send-code",
    "text=获取验证码", "text=发送验证码",
]
CODE_INPUT_SELECTORS = [
    "input[name='code']", "input[placeholder*='验证码']", "#code", "input[maxlength='6']",
]
LOGIN_BTN_SELECTORS = [
    ".btn-login", "button:has-text('登录')", "button[type='submit']", ".btn-sign",
]
# 仅匹配安全验证/滑块, 不含裸 "验证码" (它是合法的输入框文案)
CAPTCHA_MARKERS = ["geetest", "captcha", "nc_iconfont", "安全验证", "滑块", "拖动滑", "向右滑"]


def _mask_phone(phone: str) -> str:
    p = (phone or "").strip()
    if len(p) >= 7:
        return p[:3] + "****" + p[-4:]
    return "***"


async def _probe_fill(page: Page, selectors: List[str], value: str, timeout: int = 2500) -> Optional[str]:
    for sel in selectors:
        try:
            loc = page.locator(sel).first
            if await loc.count() and await loc.is_visible():
                await loc.fill(value, timeout=timeout)
                return sel
        except Exception:
            continue
    return None


async def _probe_click(page: Page, selectors: List[str], timeout: int = 2500) -> Optional[str]:
    for sel in selectors:
        try:
            loc = page.locator(sel).first
            if await loc.count() and await loc.is_visible():
                await loc.click(timeout=timeout, force=True)
                return sel
        except Exception:
            continue
    return None


async def _has_captcha(page: Page) -> bool:
    try:
        html = (await page.content()).lower()
    except Exception:
        return False
    return any(m.lower() in html for m in CAPTCHA_MARKERS)


async def _fallback_to_qr(message: str) -> Dict[str, Any]:
    """短信登录受阻 (滑块/安全验证) 时回退到二维码 + 飞书。"""
    bm = await get_browser_manager()
    try:
        login_context = await bm.get_login_context()
        qr = await acquire_qr_result(login_context, force_refresh=True)
        feishu = None
        if qr.get("qrcode_base64") and not qr.get("qr_cache_hit"):
            feishu = await send_qr_to_feishu(qr.get("qrcode_base64", ""), qr.get("image_type", "png"))
        if qr.get("qrcode_base64"):
            await start_background_wait_for_login(timeout=240)
        return {
            "success": False,
            "status": "captcha",
            "fallback": "qrcode",
            "message": message,
            "qrcode_image": {
                "image_data": qr.get("qrcode_base64", ""),
                "image_type": qr.get("image_type", "png"),
                "description": qr.get("description", "请使用Boss直聘APP扫描二维码登录"),
            },
            "feishu_delivery": feishu,
        }
    except Exception as e:
        logger.warning("fallback_to_qr failed: %s", e)
        return {"success": False, "status": "captcha", "fallback": "qrcode", "message": f"{message} (二维码兜底失败: {e})"}


async def _collect_login_cookie(login_context: BrowserContext, bm: "BrowserManager") -> Optional[str]:
    try:
        cookies = await login_context.cookies()
        cookie_str = _format_zhipin_cookie_from_cookie_list(cookies)
        if cookie_str:
            return cookie_str
    except Exception:
        pass
    return await bm.transfer_login_cookies()


async def login_send_sms(phone: Optional[str] = None) -> Dict[str, Any]:
    """第一步: 打开登录页, 填手机号, 点“获取验证码”, 保活登录页等待验证码。"""
    phone = (phone or os.getenv("BOSS_PHONE", "")).strip()
    if not phone:
        return {"success": False, "status": "no_phone", "message": "未配置 BOSS_PHONE 手机号"}

    bm = await get_browser_manager()
    page = await bm.get_persistent_login_page(force_new=True)

    nav_ok = False
    for url in (LOGIN_URL_WEB, LOGIN_URL_OLD):
        try:
            await page.goto(url, wait_until="domcontentloaded", timeout=45000)
            await page.wait_for_timeout(1500)
            if not (page.url or "").startswith("about:blank"):
                nav_ok = True
                break
        except Exception as e:
            logger.warning("login_send_sms goto %s failed: %s", url, e)
    if not nav_ok:
        return {"success": False, "status": "nav_failed", "message": "无法打开 BOSS 登录页"}

    # 切到短信登录 tab (新版登录页默认即短信, 失败可忽略)
    await _probe_click(page, SMS_TAB_SELECTORS)
    await page.wait_for_timeout(500)

    filled = await _probe_fill(page, PHONE_INPUT_SELECTORS, phone)
    if not filled:
        if await _has_captcha(page):
            return await _fallback_to_qr("短信登录受阻 (检测到安全验证), 已回退二维码登录")
        return {"success": False, "status": "phone_input_not_found", "message": "未找到手机号输入框 (登录页结构可能已变化)"}

    clicked = await _probe_click(page, SEND_CODE_SELECTORS)
    if not clicked:
        return {"success": False, "status": "send_button_not_found", "message": "未找到获取验证码按钮"}

    await page.wait_for_timeout(1500)

    if await _has_captcha(page):
        return await _fallback_to_qr("发送验证码触发安全验证, 已回退二维码登录")

    masked = _mask_phone(phone)
    return {
        "success": True,
        "status": "sms_sent",
        "phone_masked": masked,
        "message": f"已向 {masked} 发送验证码。请向用户索取验证码后调用 boss.login_submit_sms(code=...) 完成登录。",
    }


async def login_submit_sms(code: str, timeout: int = 60) -> Dict[str, Any]:
    """第二步: 在保活登录页填入验证码, 点登录, 等待登录态并提取/缓存 cookie。"""
    code = (code or "").strip()
    if not code:
        return {"success": False, "status": "no_code", "message": "缺少验证码 code"}

    bm = await get_browser_manager()
    page = bm.login_page
    ttl = _get_env_delay_seconds("BOSS_LOGIN_PAGE_TTL", 300.0)
    if page is None or (time.time() - bm.login_page_at) > ttl:
        return {"success": False, "status": "expired", "message": "登录会话已过期, 请重新调用 boss.login_send_sms"}
    try:
        if page.is_closed():
            return {"success": False, "status": "expired", "message": "登录页已关闭, 请重新调用 boss.login_send_sms"}
    except Exception:
        return {"success": False, "status": "expired", "message": "登录页不可用, 请重新调用 boss.login_send_sms"}

    filled = await _probe_fill(page, CODE_INPUT_SELECTORS, code)
    if not filled:
        return {"success": False, "status": "code_input_not_found", "message": "未找到验证码输入框"}

    await _probe_click(page, LOGIN_BTN_SELECTORS)

    login_context = await bm.get_login_context()
    start = asyncio.get_event_loop().time()
    while (asyncio.get_event_loop().time() - start) < timeout:
        await page.wait_for_timeout(1500)
        try:
            cur = page.url or ""
        except Exception:
            cur = ""
        markers_ok = "/web/geek/" in cur and "login" not in cur
        if not markers_ok:
            for sel in [".user-info", ".nav-figure", ".geek-name", ".user-nav"]:
                try:
                    if await page.query_selector(sel) is not None:
                        markers_ok = True
                        break
                except Exception:
                    continue

        cookie_str = await _collect_login_cookie(login_context, bm)
        if markers_ok and cookie_str:
            cache_login_cookie(cookie_str)
            return {"success": True, "status": "login_ok", "cookie": cookie_str, "cookie_saved": True}

        # 验证码错误 / 安全验证
        try:
            txt = await page.evaluate("() => document.body ? document.body.innerText : ''")
            if isinstance(txt, str):
                for kw in ["验证码错误", "验证码不正确", "验证码已过期", "请输入正确"]:
                    if kw in txt:
                        return {"success": False, "status": "bad_code", "message": "验证码错误或已过期, 请重新获取"}
        except Exception:
            pass
        if await _has_captcha(page):
            return {"success": False, "status": "captcha", "message": "登录触发安全验证, 请改用二维码登录 (boss.get_qrcode)"}

    cookie_str = await _collect_login_cookie(login_context, bm)
    if cookie_str:
        cache_login_cookie(cookie_str)
        return {"success": True, "status": "login_ok", "cookie": cookie_str, "cookie_saved": True}
    return {"success": False, "status": "timeout", "message": "登录超时, 请重试或改用二维码登录"}


async def start_background_wait_for_login(timeout: int = 240) -> None:
    global login_wait_task

    async with login_wait_lock:
        if login_wait_task is not None and not login_wait_task.done():
            return

    async def _runner() -> None:
        try:
            bm = await get_browser_manager()
            ctx = await bm.get_login_context()
            result = await wait_for_login(ctx, timeout)
            if result.get("success"):
                cookie = result.get("cookie")
                cache_login_cookie(cookie)
                logger.info("Background wait_login captured cookie successfully")
            else:
                logger.info("Background wait_login finished without success: %s", result.get("message"))
        except Exception as e:
            logger.warning(f"Background wait_login task failed: {e}")

    login_wait_task = asyncio.create_task(_runner())


@app.post("/")
async def handle_mcp_request(request: Request):
    req_data = await request.json()
    method = req_data.get("method")
    params = req_data.get("params", {})
    rpc_id = req_data.get("id")

    if method in ["boss.search_jobs", "boss.get_job", "boss.check_auth", "boss.apply_job"]:
        if not await rate_limiter.wait_and_acquire(timeout=30.0):
            logger.warning(f"Rate limit exceeded for method: {method}")
            return {"jsonrpc": "2.0", "error": {"code": -32603, "message": "Rate limit exceeded. Please try again later."}, "id": rpc_id}

    cookie = params.get("cookie")
    

    bm = await get_browser_manager()

    def _auth_required_sms(message: str) -> Dict[str, Any]:
        return {
            "success": False,
            "status": "auth_required",
            "login_method": "sms",
            "message": message,
            "has_cached_cookie": read_cached_login_cookie() is not None,
        }

    try:
        result = None

        # ---- 登录相关方法 (无需 cookie) ----
        if method == "boss.login_send_sms":
            result = await login_send_sms(params.get("phone"))
            return {"jsonrpc": "2.0", "result": result, "id": rpc_id}
        elif method == "boss.login_submit_sms":
            result = await login_submit_sms(params.get("code", ""), params.get("timeout", 60))
            return {"jsonrpc": "2.0", "result": result, "id": rpc_id}
        elif method == "boss.get_qrcode":
            force_refresh = bool(params.get("force_refresh", False))
            if force_refresh:
                try:
                    await bm.reset_login_context()
                    logger.info("force_refresh: login_context reset before QR capture")
                except Exception as reset_exc:
                    logger.warning("force_refresh reset failed: %s", reset_exc)
            context = await bm.get_login_context()
            result = await acquire_qr_result(context, force_refresh=force_refresh)
            should_start_wait = False
            if not result.get("is_login") and result.get("qrcode_base64"):
                should_start_wait = True
                if not result.get("qr_cache_hit"):
                    feishu_result = await send_qr_to_feishu(
                        result.get("qrcode_base64", ""),
                        result.get("image_type", "png"),
                    )
                    result["feishu_delivery"] = feishu_result
                else:
                    result["feishu_delivery"] = {"sent": False, "reason": "qr_cache_hit"}
            elif result.get("is_login"):
                should_start_wait = True
            if should_start_wait:
                await start_background_wait_for_login(timeout=240)
                result["wait_login_background_started"] = True
            else:
                result["wait_login_background_started"] = False
            result["has_cached_cookie"] = read_cached_login_cookie() is not None
            return {"jsonrpc": "2.0", "result": result, "id": rpc_id}
        elif method == "boss.wait_login":
            context = await bm.get_login_context()
            result = await wait_for_login(context, params.get("timeout", 120))
            return {"jsonrpc": "2.0", "result": result, "id": rpc_id}

        # ---- 业务方法 (需要有效 cookie; 缺失/失效则提示短信登录) ----
        biz_cookie = cookie or read_cached_login_cookie()
        if not biz_cookie:
            return {"jsonrpc": "2.0", "result": _auth_required_sms(
                "BOSS 未登录，请调用 boss.login_send_sms 发起短信登录"), "id": rpc_id}

        if method == "boss.search_jobs":
            context = await bm.get_cookie_context(biz_cookie)
            scraper = BossScraper(context)
            if not await scraper.check_auth():
                result = _auth_required_sms("BOSS cookie 失效，请调用 boss.login_send_sms 重新登录")
            else:
                result = await scraper.search_jobs(params.get("query", ""), params.get("city", ""), params.get("limit", 10))
        elif method == "boss.get_job":
            context = await bm.get_cookie_context(biz_cookie)
            scraper = BossScraper(context)
            if not await scraper.check_auth():
                result = _auth_required_sms("BOSS cookie 失效，请调用 boss.login_send_sms 重新登录")
            else:
                result = await scraper.get_job_detail(params.get("job_id", ""))
        elif method == "boss.check_auth":
            context = await bm.get_cookie_context(biz_cookie)
            scraper = BossScraper(context)
            result = {"is_login": await scraper.check_auth()}
        elif method == "boss.apply_job":
            context = await bm.get_cookie_context(biz_cookie)
            scraper = BossScraper(context)
            if not await scraper.check_auth():
                result = _auth_required_sms("BOSS cookie 失效，请调用 boss.login_send_sms 重新登录")
            else:
                result = await scraper.apply_job(params.get("job_id", ""))
        else:
            return {"jsonrpc": "2.0", "error": {"code": -32601, "message": f"Method {method} not found"}, "id": rpc_id}

        return {"jsonrpc": "2.0", "result": result, "id": rpc_id}

    except Exception as e:
        logger.error(f"MCP Error: {e}")
        return {"jsonrpc": "2.0", "error": {"code": -32603, "message": str(e)}, "id": rpc_id}

if __name__ == "__main__":
    host = os.getenv("BOSS_MCP_HOST", "0.0.0.0")
    port = int(os.getenv("BOSS_MCP_PORT", "8083"))
    logger.info(f"Starting Boss MCP on {host}:{port}")
    uvicorn.run(app, host=host, port=port)
