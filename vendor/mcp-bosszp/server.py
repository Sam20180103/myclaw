import asyncio
import json
import logging
import base64
import time
from typing import Optional, List, Dict, Any
from fastapi import FastAPI, Request, HTTPException
from playwright.async_api import async_playwright, Browser, BrowserContext
from playwright_stealth import stealth_async
from bs4 import BeautifulSoup
import uvicorn

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger("boss-mcp")

app = FastAPI(title="BossZhipin MCP Bridge")


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


class BrowserManager:
    _instance: Optional['BrowserManager'] = None
    _lock = asyncio.Lock()

    def __init__(self):
        self.pw = None
        self.browser: Optional[Browser] = None
        self.login_context: Optional[BrowserContext] = None
        self.cookie_context: Optional[BrowserContext] = None

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
                    self.browser = await self.pw.chromium.launch(headless=True)
                self.login_context = await self.browser.new_context(
                    user_agent="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36",
                    viewport={'width': 1920, 'height': 1080}
                )
                page = await self.login_context.new_page()
                await stealth_async(page)
                await page.goto("https://www.zhipin.com", wait_until="networkidle")
                await page.close()
            return self.login_context

    async def get_cookie_context(self, cookie_str: str) -> BrowserContext:
        async with self._lock:
            cookies = []
            for item in cookie_str.split(';'):
                if '=' in item:
                    name, value = item.strip().split('=', 1)
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
                self.browser = await self.pw.chromium.launch(headless=True)

            self.cookie_context = await self.browser.new_context(
                user_agent="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36",
                viewport={'width': 1920, 'height': 1080}
            )
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

    async def close(self):
        async with self._lock:
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

    async def search_jobs(self, query: str, city: str = "", limit: int = 10) -> List[Dict]:
        page = await self.context.new_page()
        await stealth_async(page)

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

        url = f"https://www.zhipin.com/web/geek/job?query={query}&city={code}"

        logger.info(f"Searching: {url}")
        await page.goto(url, wait_until="networkidle")
        await page.wait_for_selector(".job-card-wrapper", timeout=10000)

        content = await page.content()
        soup = BeautifulSoup(content, 'html.parser')
        job_list = []

        items = soup.select(".job-card-wrapper")
        for item in items[:limit]:
            try:
                link = item.select_one("a.job-card-left")
                if not link:
                    continue
                job_id = link['href'].split('/')[-1].split('.')[0]

                job_list.append({
                    "job_id": job_id,
                    "job_title": item.select_one(".job-name").text.strip(),
                    "salary": item.select_one(".salary").text.strip(),
                    "company": item.select_one(".company-name").text.strip(),
                    "city": item.select_one(".job-area").text.strip(),
                    "experience": item.select_one(".tag-list").text.strip() if item.select_one(".tag-list") else ""
                })
            except Exception as e:
                logger.error(f"Error parsing job item: {e}")

        await page.close()
        return job_list

    async def get_job_detail(self, job_id: str) -> Dict:
        page = await self.context.new_page()
        await stealth_async(page)
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
        page = await self.context.new_page()
        await page.goto("https://www.zhipin.com/web/geek/user/center")
        await asyncio.sleep(2)
        is_login = "login" not in page.url
        await page.close()
        return is_login

    async def apply_job(self, job_id: str) -> Dict[str, Any]:
        page = await self.context.new_page()
        await stealth_async(page)
        
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
                        await asyncio.sleep(2)
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
    await stealth_async(page)

    url = "https://www.zhipin.com/web/user/?ka=header-login"
    logger.info(f"Navigating to login page: {url}")
    await page.goto(url, wait_until="networkidle")

    if "login" not in page.url:
        await page.close()
        return {"is_login": True, "message": "Already logged in"}

    await page.wait_for_selector(".qrcode-wrapper img, .login-scan-wrap img, img.qrcode", timeout=10000)

    qrcode_selectors = [
        ".qrcode-wrapper img",
        ".login-scan-wrap img",
        "img.qrcode",
        "[class*='qrcode'] img",
        ".scan-qrcode img"
    ]

    qrcode_element = None
    for selector in qrcode_selectors:
        try:
            qrcode_element = await page.query_selector(selector)
            if qrcode_element:
                logger.info(f"Found QR code with selector: {selector}")
                break
        except:
            continue

    if not qrcode_element:
        logger.warning("QR code element not found, capturing login area screenshot")
        login_area = await page.query_selector(".login-scan-wrap, .qrcode-wrapper, .login-content")
        if login_area:
            screenshot_bytes = await login_area.screenshot()
        else:
            screenshot_bytes = await page.screenshot(full_page=False)
    else:
        screenshot_bytes = await qrcode_element.screenshot()

    screenshot_base64 = base64.b64encode(screenshot_bytes).decode('utf-8')

    await page.close()

    return {
        "is_login": False,
        "qrcode_base64": screenshot_base64,
        "image_type": "png",
        "description": "请使用Boss直聘APP扫描此二维码登录",
        "message": "QR code captured successfully"
    }


async def wait_for_login(context: BrowserContext, timeout: int = 120) -> Dict[str, Any]:
    page = await context.new_page()
    await stealth_async(page)

    url = "https://www.zhipin.com/web/user/?ka=header-login"
    await page.goto(url, wait_until="networkidle")

    start_time = asyncio.get_event_loop().time()

    while True:
        elapsed = asyncio.get_event_loop().time() - start_time
        if elapsed > timeout:
            await page.close()
            return {"success": False, "message": f"Login timeout after {timeout} seconds"}

        if "login" not in page.url:
            bm = await get_browser_manager()
            cookie_str = await bm.transfer_login_cookies()
            await page.close()
            return {
                "success": True,
                "message": "Login successful",
                "cookie": cookie_str
            }

        await asyncio.sleep(2)


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

    if not cookie and method not in ["boss.get_qrcode", "boss.wait_login"]:
        logger.error("Request missing cookie")
        return {"jsonrpc": "2.0", "error": {"code": -32602, "message": "Missing cookie"}, "id": rpc_id}

    bm = await get_browser_manager()

    try:
        result = None

        if method == "boss.search_jobs":
            context = await bm.get_cookie_context(cookie)
            scraper = BossScraper(context)
            result = await scraper.search_jobs(params.get("query", ""), params.get("city", ""), params.get("limit", 10))
        elif method == "boss.get_job":
            context = await bm.get_cookie_context(cookie)
            scraper = BossScraper(context)
            result = await scraper.get_job_detail(params.get("job_id", ""))
        elif method == "boss.check_auth":
            context = await bm.get_cookie_context(cookie)
            scraper = BossScraper(context)
            result = {"is_login": await scraper.check_auth()}
        elif method == "boss.apply_job":
            context = await bm.get_cookie_context(cookie)
            scraper = BossScraper(context)
            result = await scraper.apply_job(params.get("job_id", ""))
        elif method == "boss.get_qrcode":
            context = await bm.get_login_context()
            result = await get_qrcode_base64(context)
        elif method == "boss.wait_login":
            context = await bm.get_login_context()
            result = await wait_for_login(context, params.get("timeout", 120))
        else:
            return {"jsonrpc": "2.0", "error": {"code": -32601, "message": f"Method {method} not found"}, "id": rpc_id}

        return {"jsonrpc": "2.0", "result": result, "id": rpc_id}

    except Exception as e:
        logger.error(f"MCP Error: {e}")
        return {"jsonrpc": "2.0", "error": {"code": -32603, "message": str(e)}, "id": rpc_id}


@app.on_event("shutdown")
async def shutdown_event():
    bm = await get_browser_manager()
    await bm.close()


if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=8083)
