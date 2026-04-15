import asyncio
import json
import logging
from typing import Optional, List, Dict, Any
from fastapi import FastAPI, Request, HTTPException
from playwright.async_api import async_playwright
from playwright_stealth import stealth_async
from bs4 import BeautifulSoup
import uvicorn

# 配置日志
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger("boss-mcp")

app = FastAPI(title="BossZhipin MCP Bridge")

# --- 核心逻辑：BOSS 爬虫类 ---
class BossScraper:
    def __init__(self):
        self.browser = None
        self.context = None

    async def init_browser(self, cookie_str: str):
        self.pw = await async_playwright().start()
        self.browser = await self.pw.chromium.launch(headless=True)
        
        # 转换 Cookie 格式
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
        
        self.context = await self.browser.new_context(
            user_agent="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36",
            viewport={'width': 1920, 'height': 1080}
        )
        await self.context.add_cookies(cookies)

    async def close(self):
        if self.context: await self.context.close()
        if self.browser: await self.browser.close()
        if hasattr(self, 'pw'): await self.pw.stop()

    async def search_jobs(self, query: str, city: str = "", limit: int = 10) -> List[Dict]:
        page = await self.context.new_page()
        await stealth_async(page)
        
        # 城市代码映射 (示例)
        city_codes = {"广州": "101280100", "北京": "101010100", "上海": "101020100", "深圳": "101280600"}
        code = city_codes.get(city, "101280100") # 默认广州
        
        url = f"https://www.zhipin.com/web/geek/job?query={query}&city={code}"
        
        logger.info(f"Searching: {url}")
        await page.goto(url, wait_until="networkidle")
        await asyncio.sleep(3) # 等待渲染
        
        content = await page.content()
        soup = BeautifulSoup(content, 'html.parser')
        job_list = []
        
        items = soup.select(".job-card-wrapper")
        for item in items[:limit]:
            try:
                # 修复 Job ID 提取逻辑
                link = item.select_one("a.job-card-left")
                if not link: continue
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
        await asyncio.sleep(2)
        
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
        await asyncio.sleep(3)
        is_login = "login" not in page.url
        await page.close()
        return is_login

# --- MCP JSON-RPC 接口实现 ---
@app.post("/")
async def handle_mcp_request(request: Request):
    req_data = await request.json()
    method = req_data.get("method")
    params = req_data.get("params", {})
    rpc_id = req_data.get("id")
    
    cookie = params.get("cookie")
    if not cookie:
        logger.error("Request missing cookie")
        return {"jsonrpc": "2.0", "error": {"code": -32602, "message": "Missing cookie"}, "id": rpc_id}

    scraper = BossScraper()
    try:
        await scraper.init_browser(cookie)
        
        result = None
        if method == "boss.search_jobs":
            result = await scraper.search_jobs(params.get("query", ""), params.get("city", ""), params.get("limit", 10))
        elif method == "boss.get_job":
            result = await scraper.get_job_detail(params.get("job_id", ""))
        elif method == "boss.check_auth":
            result = {"is_login": await scraper.check_auth()}
        elif method == "boss.apply_job":
            result = {"success": True, "message": "模拟投递成功（已接收投递指令）"}
        else:
            return {"jsonrpc": "2.0", "error": {"code": -32601, "message": f"Method {method} not found"}, "id": rpc_id}

        return {"jsonrpc": "2.0", "result": result, "id": rpc_id}
    
    except Exception as e:
        logger.error(f"MCP Error: {e}")
        return {"jsonrpc": "2.0", "error": {"code": -32603, "message": str(e)}, "id": rpc_id}
    finally:
        await scraper.close()

if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=8083)
