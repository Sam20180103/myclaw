"""短信登录冒烟测试。

- 无 BOSS_PHONE 时: 断言 login_send_sms 返回 status=="no_phone" (纯逻辑, 不联网)。
- 设了 BOSS_PHONE 时: 真实打开登录页发码; 若再设 BOSS_TEST_CODE 则提交验证码。

用法:
    python test_sms_login.py                         # 仅跑 no_phone 断言
    BOSS_PHONE=138xxxx python test_sms_login.py       # 真实发码
    BOSS_PHONE=138xxxx BOSS_TEST_CODE=472913 python test_sms_login.py  # 发码+提交
"""
import asyncio
import importlib.util
import json
import os
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
SERVER_PATH = SCRIPT_DIR / "server.py"


def load_server_module():
    spec = importlib.util.spec_from_file_location("boss_server", str(SERVER_PATH))
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Failed to load module from {SERVER_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


async def main() -> int:
    server = load_server_module()

    # 1) no_phone 路径 (无需浏览器/网络)
    saved = os.environ.pop("BOSS_PHONE", None)
    try:
        res = await server.login_send_sms(None)
        print("no_phone result:", json.dumps(res, ensure_ascii=False))
        assert res.get("status") == "no_phone", f"expected no_phone, got {res}"
        print("PASS: no_phone path")
    finally:
        if saved is not None:
            os.environ["BOSS_PHONE"] = saved

    # 2) 真实流程 (可选)
    phone = os.getenv("BOSS_PHONE", "").strip()
    if not phone:
        print("BOSS_PHONE 未设置, 跳过真实发码测试。")
        return 0

    bm = await server.get_browser_manager()
    try:
        send = await server.login_send_sms(phone)
        print("login_send_sms:", json.dumps(send, ensure_ascii=False))

        code = os.getenv("BOSS_TEST_CODE", "").strip()
        if send.get("status") == "sms_sent" and code:
            submit = await server.login_submit_sms(code)
            redacted = {k: ("***" if k == "cookie" else v) for k, v in submit.items()}
            print("login_submit_sms:", json.dumps(redacted, ensure_ascii=False))
        elif send.get("status") == "sms_sent":
            print("已发码, 设置 BOSS_TEST_CODE=<验证码> 再次运行以提交。")
        return 0
    except Exception as e:
        print(f"Test failed: {e}", file=sys.stderr)
        return 1
    finally:
        await bm.close()


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
