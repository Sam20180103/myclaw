"""小红书 (Xiaohongshu) MCP 服务器 — Playwright 模拟用户操作。

镜像 vendor/mcp-bosszp/server.py 的结构与登录范式:
  - BrowserManager 单例 (Chromium + stealth + 指纹 + 代理)
  - 手机号+短信验证码 两步登录 (login_send_sms / login_submit_sms) + 保活登录页 + cookie 缓存
  - JSON-RPC dispatch (FastAPI)

方法:
  xhs.login_send_sms(phone?)        发送验证码 (服务端读 XHS_PHONE)
  xhs.login_submit_sms(code)        提交验证码完成登录, 提取并缓存 cookie
  xhs.check_auth(cookie?)           校验登录态
  xhs.get_hot(limit=10)             热门 Top N 笔记
  xhs.get_note(note_id)             读单篇笔记
  xhs.search(query, limit)          搜索笔记
  xhs.get_user(user_id)             用户信息
  xhs.publish({title, body, topics, image_paths})  发布图文笔记

env: XHS_PHONE, XHS_LOGIN_PAGE_TTL(默认300), XHS_COOKIE_CACHE_PATH, XHS_MCP_HOST/PORT(默认 0.0.0.0:8085),
     XHS_MCP_PROXY/HTTPS_PROXY/...

⚠️ 选择器为 best-effort, 需对照小红书线上 DOM 校准。自动化发布违反平台规则, 有封号风险, 请用小号低频。
"""
import asyncio
import json
import logging
import os
import time
from typing import Optional, List, Dict, Any

from fastapi import FastAPI, Request
from playwright.async_api import async_playwright, Browser, BrowserContext, Page
from playwright_stealth import Stealth
import uvicorn

logging.basicConfig(level=logging.INFO, format="%(asctime)s - %(levelname)s - %(message)s")
logger = logging.getLogger("xhs-mcp")

MAC_CHROME_UA = (
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_5) "
    "AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.7680.178 Safari/537.36"
)
LOGIN_URL = "https://www.xiaohongshu.com"
EXPLORE_URL = "https://www.xiaohongshu.com/explore"
PUBLISH_URL = "https://creator.xiaohongshu.com/publish/publish"

COOKIE_CACHE_PATH = os.getenv(
    "XHS_COOKIE_CACHE_PATH", os.path.join(os.path.dirname(__file__), "xhs_login_cookie.json")
)

# --- best-effort 选择器 (需对照线上 DOM 校准) ---
PHONE_INPUT_SELECTORS = [
    "input[placeholder*='手机号']", "input[type='tel']", ".phone input", "#phone",
]
SEND_CODE_SELECTORS = [
    "button:has-text('获取验证码')", "button:has-text('发送验证码')", ".send-code",
    "text=获取验证码", "text=发送验证码",
]
CODE_INPUT_SELECTORS = [
    "input[placeholder*='验证码']", "input[maxlength='6']", "#code",
]
LOGIN_BTN_SELECTORS = [
    "button:has-text('登录')", ".login-btn", "button[type='submit']", ".submit",
]
LOGIN_STATE_SELECTORS = [".user", ".avatar", ".reds-avatar", ".user-info", "[class*='user-avatar']"]
CAPTCHA_MARKERS = ["geetest", "captcha", "nc_iconfont", "安全验证", "滑块", "拖动滑", "向右滑"]
# 发布页选择器
PUB_IMAGE_INPUT_SELECTORS = ["input[type='file']"]
PUB_TITLE_SELECTORS = ["input[placeholder*='标题']", ".title input", "input.title"]
PUB_BODY_SELECTORS = ["[contenteditable='true']", "textarea", ".content-input"]
PUB_SUBMIT_SELECTORS = ["button:has-text('发布')", ".publish-btn", "button:has-text('发布笔记')"]


def _get_env_float(name: str, default: float) -> float:
    raw = os.getenv(name)
    if raw is None:
        return default
    try:
        v = float(raw)
        return v if v >= 0 else default
    except ValueError:
        return default


def get_browser_proxy() -> Optional[Dict[str, str]]:
    for key in ("XHS_MCP_PROXY", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"):
        val = os.getenv(key)
        if val:
            return {"server": val}
    return None


def browser_launch_kwargs() -> Dict[str, Any]:
    kwargs: Dict[str, Any] = {
        "headless": True,
        "args": [
            "--disable-blink-features=AutomationControlled",
            "--no-sandbox",
            "--window-size=1920,1080",
        ],
    }
    proxy = get_browser_proxy()
    if proxy:
        kwargs["proxy"] = proxy
    return kwargs


def browser_context_kwargs() -> Dict[str, Any]:
    return {
        "user_agent": MAC_CHROME_UA,
        "viewport": {"width": 1920, "height": 1080},
        "locale": os.getenv("XHS_FP_LOCALE", "zh-CN"),
        "timezone_id": os.getenv("XHS_FP_TIMEZONE", "Asia/Shanghai"),
    }


def build_fingerprint_init_script() -> str:
    return """
Object.defineProperty(navigator, 'webdriver', { get: () => undefined });
Object.defineProperty(navigator, 'platform', { get: () => 'MacIntel' });
Object.defineProperty(navigator, 'languages', { get: () => ['zh-CN', 'zh', 'en'] });
Object.defineProperty(navigator, 'hardwareConcurrency', { get: () => 8 });
"""


# --- cookie 缓存 (内存 + 文件) ---
cached_cookie: Optional[str] = None
cached_cookie_at: float = 0.0


def save_cookie_file(cookie: Optional[str]) -> None:
    if not cookie:
        return
    try:
        with open(COOKIE_CACHE_PATH, "w", encoding="utf-8") as f:
            json.dump({"cookie": cookie, "saved_at": int(time.time())}, f, ensure_ascii=False)
    except Exception as e:
        logger.warning("save cookie file failed: %s", e)


def read_cookie_file(max_age_seconds: int = 86400) -> Optional[str]:
    if not os.path.exists(COOKIE_CACHE_PATH):
        return None
    try:
        with open(COOKIE_CACHE_PATH, "r", encoding="utf-8") as f:
            payload = json.load(f)
        cookie = payload.get("cookie")
        saved_at = int(payload.get("saved_at", 0))
        if not cookie or not saved_at or (time.time() - saved_at) > max_age_seconds:
            return None
        return cookie
    except Exception:
        return None


def cache_cookie(cookie: Optional[str]) -> None:
    global cached_cookie, cached_cookie_at
    if cookie:
        cached_cookie = cookie
        cached_cookie_at = time.time()
        save_cookie_file(cookie)


def read_cached_cookie(max_age_seconds: int = 86400) -> Optional[str]:
    if cached_cookie and (time.time() - cached_cookie_at) <= max_age_seconds:
        return cached_cookie
    f = read_cookie_file(max_age_seconds)
    if f:
        cache_cookie(f)
        return f
    return None


def _format_cookie(cookies: List[Dict[str, Any]]) -> Optional[str]:
    parts = []
    for c in cookies:
        domain = (c.get("domain") or "").lower()
        if "xiaohongshu" not in domain:
            continue
        name, value = c.get("name"), c.get("value")
        if name and value:
            parts.append(f"{name}={value}")
    return "; ".join(parts) if parts else None


def _mask_phone(phone: str) -> str:
    p = (phone or "").strip()
    return p[:3] + "****" + p[-4:] if len(p) >= 7 else "***"


class BrowserManager:
    _instance: Optional["BrowserManager"] = None
    _lock = asyncio.Lock()

    def __init__(self):
        self.pw = None
        self.browser: Optional[Browser] = None
        self.login_context: Optional[BrowserContext] = None
        self.cookie_context: Optional[BrowserContext] = None
        self.login_page: Optional[Page] = None
        self.login_page_at: float = 0.0

    @classmethod
    async def get_instance(cls) -> "BrowserManager":
        async with cls._lock:
            if cls._instance is None:
                cls._instance = cls()
            return cls._instance

    async def _ensure_browser(self):
        if self.browser is None:
            if self.pw is None:
                self.pw = await async_playwright().start()
            self.browser = await self.pw.chromium.launch(**browser_launch_kwargs())

    async def get_login_context(self) -> BrowserContext:
        async with self._lock:
            if self.login_context is None:
                await self._ensure_browser()
                self.login_context = await self.browser.new_context(**browser_context_kwargs())
                await self.login_context.add_init_script(build_fingerprint_init_script())
            return self.login_context

    async def get_persistent_login_page(self, force_new: bool = False) -> Page:
        ttl = _get_env_float("XHS_LOGIN_PAGE_TTL", 300.0)
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
            for item in cookie_str.split(";"):
                item = item.strip()
                if not item or "=" not in item:
                    continue
                idx = item.index("=")
                name, value = item[:idx].strip(), item[idx + 1:].strip()
                if name and value:
                    cookies.append({"name": name, "value": value, "domain": ".xiaohongshu.com", "path": "/"})
            if self.cookie_context:
                await self.cookie_context.close()
            await self._ensure_browser()
            self.cookie_context = await self.browser.new_context(**browser_context_kwargs())
            await self.cookie_context.add_init_script(build_fingerprint_init_script())
            if cookies:
                await self.cookie_context.add_cookies(cookies)
            return self.cookie_context

    async def transfer_login_cookies(self) -> Optional[str]:
        async with self._lock:
            if self.login_context is None:
                return None
            try:
                return _format_cookie(await self.login_context.cookies())
            except Exception:
                return None


browser_manager: Optional[BrowserManager] = None


async def get_browser_manager() -> BrowserManager:
    global browser_manager
    if browser_manager is None:
        browser_manager = await BrowserManager.get_instance()
    return browser_manager


# --- 选择器探测助手 ---
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


# --- 登录 ---
async def login_send_sms(phone: Optional[str] = None) -> Dict[str, Any]:
    phone = (phone or os.getenv("XHS_PHONE", "")).strip()
    if not phone:
        return {"success": False, "status": "no_phone", "message": "未配置 XHS_PHONE 手机号"}
    bm = await get_browser_manager()
    page = await bm.get_persistent_login_page(force_new=True)
    try:
        await page.goto(LOGIN_URL, wait_until="domcontentloaded", timeout=45000)
        await page.wait_for_timeout(2000)
    except Exception as e:
        return {"success": False, "status": "nav_failed", "message": f"打开登录页失败: {e}"}

    # 触发登录弹窗 (部分情况首页需点击登录入口)
    await _probe_click(page, ["text=登录", "button:has-text('登录')", ".login-btn"])
    await page.wait_for_timeout(800)

    if not await _probe_fill(page, PHONE_INPUT_SELECTORS, phone):
        if await _has_captcha(page):
            return {"success": False, "status": "captcha", "message": "登录受阻(安全验证), 请改用 cookie 注入或稍后再试"}
        return {"success": False, "status": "phone_input_not_found", "message": "未找到手机号输入框(登录页结构可能已变化)"}

    if not await _probe_click(page, SEND_CODE_SELECTORS):
        return {"success": False, "status": "send_button_not_found", "message": "未找到获取验证码按钮"}

    await page.wait_for_timeout(1500)
    if await _has_captcha(page):
        return {"success": False, "status": "captcha", "message": "发送验证码触发安全验证"}

    masked = _mask_phone(phone)
    return {
        "success": True,
        "status": "sms_sent",
        "phone_masked": masked,
        "message": f"已向 {masked} 发送验证码, 请向用户索取验证码后调用 xhs.login_submit_sms(code=...)",
    }


async def login_submit_sms(code: str, timeout: int = 60) -> Dict[str, Any]:
    code = (code or "").strip()
    if not code:
        return {"success": False, "status": "no_code", "message": "缺少验证码 code"}
    bm = await get_browser_manager()
    page = bm.login_page
    ttl = _get_env_float("XHS_LOGIN_PAGE_TTL", 300.0)
    if page is None or (time.time() - bm.login_page_at) > ttl:
        return {"success": False, "status": "expired", "message": "登录会话已过期, 请重新 login_send_sms"}
    try:
        if page.is_closed():
            return {"success": False, "status": "expired", "message": "登录页已关闭, 请重新 login_send_sms"}
    except Exception:
        return {"success": False, "status": "expired", "message": "登录页不可用, 请重新 login_send_sms"}

    if not await _probe_fill(page, CODE_INPUT_SELECTORS, code):
        return {"success": False, "status": "code_input_not_found", "message": "未找到验证码输入框"}
    await _probe_click(page, LOGIN_BTN_SELECTORS)

    login_context = await bm.get_login_context()
    start = asyncio.get_event_loop().time()
    while (asyncio.get_event_loop().time() - start) < timeout:
        await page.wait_for_timeout(1500)
        ok = False
        for sel in LOGIN_STATE_SELECTORS:
            try:
                if await page.query_selector(sel) is not None:
                    ok = True
                    break
            except Exception:
                continue
        cookie_str = None
        try:
            cookie_str = _format_cookie(await login_context.cookies())
        except Exception:
            cookie_str = None
        # web_session 出现即视为登录
        if cookie_str and ("web_session" in cookie_str or ok):
            cache_cookie(cookie_str)
            return {"success": True, "status": "login_ok", "cookie": cookie_str, "cookie_saved": True}
        try:
            txt = await page.evaluate("() => document.body ? document.body.innerText : ''")
            if isinstance(txt, str):
                for kw in ["验证码错误", "验证码不正确", "验证码已过期"]:
                    if kw in txt:
                        return {"success": False, "status": "bad_code", "message": "验证码错误或已过期"}
        except Exception:
            pass
        if await _has_captcha(page):
            return {"success": False, "status": "captcha", "message": "登录触发安全验证"}

    cookie_str = None
    try:
        cookie_str = _format_cookie(await login_context.cookies())
    except Exception:
        pass
    if cookie_str:
        cache_cookie(cookie_str)
        return {"success": True, "status": "login_ok", "cookie": cookie_str, "cookie_saved": True}
    return {"success": False, "status": "timeout", "message": "登录超时, 请重试"}


def _auth_required() -> Dict[str, Any]:
    return {
        "success": False,
        "status": "auth_required",
        "login_method": "sms",
        "message": "小红书未登录, 请调用 xhs.login_send_sms 发起短信登录",
        "has_cached_cookie": read_cached_cookie() is not None,
    }


async def _authed_context(cookie: Optional[str]) -> Optional[BrowserContext]:
    bm = await get_browser_manager()
    biz_cookie = cookie or read_cached_cookie()
    if not biz_cookie:
        return None
    return await bm.get_cookie_context(biz_cookie)


async def check_auth(cookie: Optional[str]) -> bool:
    ctx = await _authed_context(cookie)
    if ctx is None:
        return False
    try:
        cookies = await ctx.cookies()
        return any(c.get("name") == "web_session" and c.get("value") for c in cookies)
    except Exception:
        return False


async def get_hot(limit: int = 10, cookie: Optional[str] = None) -> Dict[str, Any]:
    ctx = await _authed_context(cookie)
    if ctx is None:
        return _auth_required()
    page = await ctx.new_page()
    await Stealth().apply_stealth_async(page)
    try:
        await page.goto(EXPLORE_URL, wait_until="domcontentloaded", timeout=45000)
        await page.wait_for_timeout(2500)
        # 滚动加载
        for _ in range(3):
            await page.mouse.wheel(0, 3000)
            await page.wait_for_timeout(800)
        # best-effort 抽取笔记卡片
        notes = await page.evaluate(
            """(limit) => {
                const out = [];
                const cards = document.querySelectorAll("a[href*='/explore/'], section.note-item, .note-item");
                for (const c of cards) {
                    const link = c.href || (c.querySelector('a') && c.querySelector('a').href) || '';
                    const title = (c.querySelector("[class*='title'], .title, span") || {}).innerText || '';
                    if (link) out.push({ url: link, title: (title || '').trim() });
                    if (out.length >= limit) break;
                }
                return out;
            }""",
            limit,
        )
        return {"success": True, "count": len(notes or []), "notes": notes or []}
    except Exception as e:
        return {"success": False, "status": "error", "message": f"get_hot 失败: {e}"}
    finally:
        try:
            await page.close()
        except Exception:
            pass


async def publish(title: str, body: str, topics: List[str], image_paths: List[str], cookie: Optional[str]) -> Dict[str, Any]:
    if not image_paths:
        return {"success": False, "status": "no_images", "message": "发布需要至少一张图片"}
    ctx = await _authed_context(cookie)
    if ctx is None:
        return _auth_required()
    page = await ctx.new_page()
    await Stealth().apply_stealth_async(page)
    try:
        await page.goto(PUBLISH_URL, wait_until="domcontentloaded", timeout=60000)
        await page.wait_for_timeout(2500)
        if "login" in (page.url or "") or await _has_captcha(page):
            return _auth_required()

        # 上传图片
        file_input = None
        for sel in PUB_IMAGE_INPUT_SELECTORS:
            try:
                loc = page.locator(sel).first
                if await loc.count():
                    file_input = loc
                    break
            except Exception:
                continue
        if file_input is None:
            return {"success": False, "status": "upload_input_not_found", "message": "未找到图片上传入口(发布页结构可能已变化)"}
        await file_input.set_input_files(image_paths)
        await page.wait_for_timeout(4000)

        # 标题 + 正文 (正文追加话题标签)
        await _probe_fill(page, PUB_TITLE_SELECTORS, title)
        full_body = body or ""
        if topics:
            full_body = full_body + "\n" + " ".join(f"#{t}" for t in topics)
        await _probe_fill(page, PUB_BODY_SELECTORS, full_body)
        await page.wait_for_timeout(1000)

        if not await _probe_click(page, PUB_SUBMIT_SELECTORS):
            return {"success": False, "status": "submit_not_found", "message": "未找到发布按钮"}
        await page.wait_for_timeout(4000)
        return {"success": True, "status": "published", "title": title, "images": len(image_paths)}
    except Exception as e:
        return {"success": False, "status": "error", "message": f"publish 失败: {e}"}
    finally:
        try:
            await page.close()
        except Exception:
            pass


# --- 简单读方法 (best-effort, 兼容现有 XhsReachTool) ---
async def get_note(note_id: str, cookie: Optional[str]) -> Dict[str, Any]:
    ctx = await _authed_context(cookie)
    if ctx is None:
        return _auth_required()
    page = await ctx.new_page()
    try:
        await page.goto(f"https://www.xiaohongshu.com/explore/{note_id}", wait_until="domcontentloaded", timeout=45000)
        await page.wait_for_timeout(2000)
        text = await page.evaluate("() => document.body ? document.body.innerText.slice(0, 4000) : ''")
        return {"success": True, "note_id": note_id, "text": text}
    except Exception as e:
        return {"success": False, "message": str(e)}
    finally:
        try:
            await page.close()
        except Exception:
            pass


async def search(query: str, limit: int, cookie: Optional[str]) -> Dict[str, Any]:
    ctx = await _authed_context(cookie)
    if ctx is None:
        return _auth_required()
    page = await ctx.new_page()
    try:
        from urllib.parse import quote
        await page.goto(f"https://www.xiaohongshu.com/search_result?keyword={quote(query)}", wait_until="domcontentloaded", timeout=45000)
        await page.wait_for_timeout(2500)
        notes = await page.evaluate(
            """(limit) => {
                const out = [];
                const cards = document.querySelectorAll("a[href*='/explore/']");
                for (const c of cards) { if (c.href) out.push({url: c.href, title: (c.innerText||'').trim()}); if (out.length>=limit) break; }
                return out;
            }""",
            limit,
        )
        return {"success": True, "count": len(notes or []), "notes": notes or []}
    except Exception as e:
        return {"success": False, "message": str(e)}
    finally:
        try:
            await page.close()
        except Exception:
            pass


async def get_user(user_id: str, cookie: Optional[str]) -> Dict[str, Any]:
    ctx = await _authed_context(cookie)
    if ctx is None:
        return _auth_required()
    page = await ctx.new_page()
    try:
        await page.goto(f"https://www.xiaohongshu.com/user/profile/{user_id}", wait_until="domcontentloaded", timeout=45000)
        await page.wait_for_timeout(2000)
        text = await page.evaluate("() => document.body ? document.body.innerText.slice(0, 2000) : ''")
        return {"success": True, "user_id": user_id, "text": text}
    except Exception as e:
        return {"success": False, "message": str(e)}
    finally:
        try:
            await page.close()
        except Exception:
            pass


app = FastAPI()


@app.get("/health")
async def health():
    return {"status": "ok", "service": "xhs-mcp"}


@app.post("/")
async def handle_mcp_request(request: Request):
    req = await request.json()
    method = req.get("method")
    params = req.get("params", {})
    rpc_id = req.get("id")
    cookie = params.get("cookie")

    try:
        if method == "xhs.login_send_sms":
            result = await login_send_sms(params.get("phone"))
        elif method == "xhs.login_submit_sms":
            result = await login_submit_sms(params.get("code", ""), params.get("timeout", 60))
        elif method == "xhs.check_auth":
            result = {"is_login": await check_auth(cookie)}
        elif method == "xhs.get_hot":
            result = await get_hot(int(params.get("limit", 10)), cookie)
        elif method == "xhs.get_note":
            result = await get_note(params.get("note_id", ""), cookie)
        elif method == "xhs.search":
            result = await search(params.get("query", ""), int(params.get("limit", 20)), cookie)
        elif method == "xhs.get_user":
            result = await get_user(params.get("user_id", ""), cookie)
        elif method == "xhs.publish":
            result = await publish(
                params.get("title", ""),
                params.get("body", ""),
                params.get("topics", []) or [],
                params.get("image_paths", []) or [],
                cookie,
            )
        else:
            return {"jsonrpc": "2.0", "error": {"code": -32601, "message": f"Method {method} not found"}, "id": rpc_id}
        return {"jsonrpc": "2.0", "result": result, "id": rpc_id}
    except Exception as e:
        logger.error("MCP error: %s", e)
        return {"jsonrpc": "2.0", "error": {"code": -32603, "message": str(e)}, "id": rpc_id}


if __name__ == "__main__":
    host = os.getenv("XHS_MCP_HOST", "0.0.0.0")
    port = int(os.getenv("XHS_MCP_PORT", "8085"))
    logger.info("Starting Xiaohongshu MCP on %s:%s", host, port)
    uvicorn.run(app, host=host, port=port)
