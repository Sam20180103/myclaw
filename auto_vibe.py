#!/usr/bin/env python3
import subprocess
import time
import sys
import os
from datetime import datetime

# --- 配置区域 ---
# 在这里定义你的任务.  根据 issues.md 、 roadmap.md以及architecture.md 继续编写代码
TASKS = [
    {
        "name": "📝 代码编写 (Coder)",
        "command": "vibe run \"根据roadmap.md以及architecture.md 继续编写代码完善功能\" --agent gemini",
        "interval": 0,  # 任务完成后等待0秒，立即进入下一个任务
        "log_file": "coder.log"
    },
    # {
    #     "name": "🧪 测试编写 (Tester)",
    #     "command": "vibe run \"为当前的代码实现编写和运行单元测试\" --agent gemini",
    #     "interval": 0,  # 任务完成后等待0秒，立即进入下一个任务
    #     "log_file": "tester.log"
    # },
    {
        "name": "🔍 代码审核 (Reviewer)",
        "command": "vibe run \"审查最近新增功能的代码，查找潜在的bug和安全问题\" --agent gemini",
        "interval": 1800,  # 审核任务完成后等待30分钟 (1800秒)，然后进入下一轮循环
        "log_file": "reviewer.log"
    }
]

# 每轮完整循环的间隔 (秒)，如果设置了可以在这里取消注释
# LOOP_INTERVAL = 3600  # 1小时

# 项目根目录（Vibe-Switch将在此目录下创建Git Worktree）
PROJECT_DIR = os.path.dirname(os.path.abspath(__file__))
# --- 配置结束 ---

def run_vibe_task(task_name, command, log_file):
    """执行一个Vibe任务并记录输出"""
    timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    log_entry = f"\n[{timestamp}] --- 开始任务: {task_name} ---\n"
    
    print(log_entry.strip())
    with open(log_file, 'a') as f:
        f.write(log_entry)

    # 构建代理环境变量
    env = os.environ.copy()
    env['http_proxy'] = 'http://127.0.0.1:8118'
    env['https_proxy'] = 'http://127.0.0.1:8118'
    # 如果需要 no_proxy，也可以添加
    # env['no_proxy'] = 'localhost,127.0.0.1'   
 
    try:
        # 在项目目录中执行命令
        process = subprocess.Popen(
            command,
            shell=True,
            cwd=PROJECT_DIR,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
          # text=True,
            bufsize=1,
            universal_newlines=True
        )
        
        # 实时输出并记录日志
        for line in process.stdout:
            print(line, end='')
            with open(log_file, 'a') as f:
                f.write(line)
        
        process.wait()
        
        timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        finish_msg = f"\n[{timestamp}] --- 完成任务: {task_name} (退出码: {process.returncode}) ---\n"
        print(finish_msg.strip())
        with open(log_file, 'a') as f:
            f.write(finish_msg)
        
        return process.returncode == 0
    except Exception as e:
        error_msg = f"执行任务时出错: {e}"
        print(error_msg)
        with open(log_file, 'a') as f:
            f.write(error_msg + "\n")
        return False

def main():
    print(f"🚀 启动Vibe自动化工作流")
    print(f"📁 项目目录: {PROJECT_DIR}")
    print("=" * 50)
    
    cycle_count = 0
    while True:
        cycle_count += 1
        print(f"\n🔄 --- 开始第 {cycle_count} 轮循环 ---")
        
        for task in TASKS:
            print(f"\n▶️  执行: {task['name']}")
            success = run_vibe_task(task["name"], task["command"], task["log_file"])
            
            if not success:
                print(f"⚠️  任务 {task['name']} 执行失败，继续执行下一个任务...")
            
            if task["interval"] > 0:
                print(f"⏳ 任务 '{task['name']}' 完成后，等待 {task['interval']} 秒...")
                time.sleep(task["interval"])
        
        print(f"\n✅ 第 {cycle_count} 轮循环完成。")
        print(f"⏳ 在开始下一轮循环前，等待所有任务的间隔设置...")
        
        # 如果希望在一整轮循环后等待，可以启用这里
        # if LOOP_INTERVAL > 0:
        #     print(f"⏳ 循环间隔: {LOOP_INTERVAL} 秒")
        #     time.sleep(LOOP_INTERVAL)

if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\n🛑 用户中断，正在停止Vibe自动化工作流...")
        sys.exit(0)
