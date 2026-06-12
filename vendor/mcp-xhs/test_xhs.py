"""小红书 MCP 冒烟测试。

- 无 XHS_PHONE: 断言 login_send_sms → status=="no_phone" (不联网)。
- 设 XHS_PHONE: 真实发码; 再设 XHS_TEST_CODE 则提交。

用法:
    python test_xhs.py
    XHS_PHONE=138xxxx python test_xhs.py
    XHS_PHONE=138xxxx XHS_TEST_CODE=123456 python test_xhs.py
"""
import asyncio
import importlib.util
import json
import os
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
SERVER_PATH = SCRIPT_DIR / "server.py"


def load_server():
    spec = importlib.util.spec_from_file_location("xhs_server", str(SERVER_PATH))
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {SERVER_PATH}")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


async def main() -> int:
    server = load_server()

    saved = os.environ.pop("XHS_PHONE", None)
    try:
        res = await server.login_send_sms(None)
        print("no_phone:", json.dumps(res, ensure_ascii=False))
        assert res.get("status") == "no_phone", f"expected no_phone, got {res}"
        print("PASS: no_phone path")
    finally:
        if saved is not None:
            os.environ["XHS_PHONE"] = saved

    phone = os.getenv("XHS_PHONE", "").strip()
    if not phone:
        print("XHS_PHONE 未设置, 跳过真实流程。")
        return 0

    bm = await server.get_browser_manager()
    try:
        send = await server.login_send_sms(phone)
        print("login_send_sms:", json.dumps(send, ensure_ascii=False))
        code = os.getenv("XHS_TEST_CODE", "").strip()
        if send.get("status") == "sms_sent" and code:
            submit = await server.login_submit_sms(code)
            redacted = {k: ("***" if k == "cookie" else v) for k, v in submit.items()}
            print("login_submit_sms:", json.dumps(redacted, ensure_ascii=False))
        return 0
    except Exception as e:
        print(f"failed: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
