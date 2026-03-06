#!/bin/bash
set -e

MODEL="${OPENCODE_MODEL:-}"
FORMAT="${OPENCODE_FORMAT:-default}"
WORKDIR="${OPENCODE_DIR:-.}"
EXTRA_OPTS="${OPENCODE_OPTS:-}"
LOG_FILE="${OPENCODE_LOG_FILE:-/tmp/opencode-loop.log}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log() {
    local level="$1"
    local msg="$2"
    local timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo -e "[$timestamp] [$level] $msg" | tee -a "$LOG_FILE"
}

log_info()  { log "INFO" "${GREEN}$1${NC}"; }
log_warn()  { log "WARN" "${YELLOW}$1${NC}"; }
log_error() { log "ERROR" "${RED}$1${NC}"; }
log_step()  { log "STEP" "${BLUE}$1${NC}"; }

show_help() {
    cat << 'EOF'
Usage: opencode-loop.sh [OPTIONS] [PROMPT]

Options:
    -h, --help              Show help
    -l, --loop              无限循环模式 (执行后继续等待输入)
    -m, --model MODEL       Model (e.g., opencode/minimax-m2.5-free)
    -f, --file FILE         Read prompts from file (one per line, then exit)
    -d, --dir DIR           Working directory
    -o, --output FILE       Output file
    -c, --continue          Continue last session
    -s, --session ID        Continue specific session
    --format FORMAT         Output format (default/json)
    --no-exec               Dry-run mode
EOF
}

PROMPT=""
INPUT_FILE=""
CONTINUE_SESSION=""
SESSION_ID=""
DRY_RUN=false
OUTPUT_FILE=""
FORCE_LOOP=false

while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            show_help
            exit 0
            ;;
        -m|--model)
            MODEL="$2"
            shift 2
            ;;
        -f|--file)
            INPUT_FILE="$2"
            shift 2
            ;;
        -d|--dir)
            WORKDIR="$2"
            shift 2
            ;;
        -o|--output)
            OUTPUT_FILE="$2"
            shift 2
            ;;
        -c|--continue)
            CONTINUE_SESSION="--continue"
            shift
            ;;
        -s|--session)
            SESSION_ID="--session $2"
            shift 2
            ;;
        --format)
            FORMAT="$2"
            shift 2
            ;;
        --no-exec)
            DRY_RUN=true
            shift
            ;;
        -l|--loop)
            FORCE_LOOP=true
            shift
            ;;
        *)
            PROMPT="$1"
            shift
            ;;
    esac
done

build_cmd() {
    local prompt="$1"
    local cmd="opencode run"
    cmd="$cmd \"$prompt\""
    [[ -n "$MODEL" ]] && cmd="$cmd --model $MODEL"
    cmd="$cmd --format $FORMAT"
    [[ -n "$EXTRA_OPTS" ]] && cmd="$cmd $EXTRA_OPTS"
    [[ -n "$CONTINUE_SESSION" ]] && cmd="$cmd $CONTINUE_SESSION"
    [[ -n "$SESSION_ID" ]] && cmd="$cmd $SESSION_ID"
    cmd="$cmd --dir $WORKDIR"
    echo "$cmd"
}

execute_prompt() {
    local prompt="$1"
    local cmd
    cmd=$(build_cmd "$prompt")
    
    log_step "执行: $prompt"
    
    if [[ "$DRY_RUN" == "true" ]]; then
        log_info "[DRY-RUN] $cmd"
        return 0
    fi
    
    log_info "命令: $cmd"
    
    if [[ -n "$OUTPUT_FILE" ]]; then
        eval "$cmd" 2>&1 | tee -a "$OUTPUT_FILE"
    else
        eval "$cmd" 2>&1
    fi
    
    local exit_code=$?
    if [[ $exit_code -eq 0 ]]; then
        log_info "✅ 执行成功"
    else
        log_error "❌ 执行失败 (exit code: $exit_code)"
    fi
    return $exit_code
}

main() {
    log_info "🚀 OpenCode Loop 启动"
    log_info "工作目录: $WORKDIR"
    log_info "输出格式: $FORMAT"
    [[ -n "$MODEL" ]] && log_info "模型: $MODEL"
    echo ""
    
    if [[ -n "$INPUT_FILE" ]]; then
        if [[ ! -f "$INPUT_FILE" ]]; then
            log_error "文件不存在: $INPUT_FILE"
            exit 1
        fi
        log_info "从文件读取: $INPUT_FILE"
        line_num=0
        while IFS= read -r line || [[ -n "$line" ]]; do
            line_num=$((line_num + 1))
            [[ -z "$line" ]] && continue
            [[ "$line" =~ ^# ]] && continue
            log_info "--- 第 $line_num 行 ---"
            execute_prompt "$line"
            echo ""
            sleep 1
        done < "$INPUT_FILE"
        log_info "📄 文件处理完成，共 $line_num 行"
        return
    fi
    
    if [[ "$FORCE_LOOP" == "true" && -n "$PROMPT" ]]; then
        log_info "🔄 无限循环模式: 自动重复执行 (Ctrl+C 退出)"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        
        while true; do
            echo -e "${BLUE}▶ 执行: $PROMPT${NC}"
            execute_prompt "$PROMPT"
            echo ""
            sleep 2
        done
        log_info "👋 退出循环"
    elif [[ -n "$PROMPT" || "$FORCE_LOOP" == "true" ]]; then
        if [[ -n "$PROMPT" ]]; then
            execute_prompt "$PROMPT"
            echo ""
        fi
        log_info "🔄 无限循环模式: 输入提示词继续 (Ctrl+C 退出)"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        
        while true; do
            echo -n -e "${BLUE}>> ${NC}"
            if IFS= read -r PROMPT; then
                if [[ -n "$PROMPT" ]]; then
                    execute_prompt "$PROMPT"
                    echo ""
                fi
            else
                break
            fi
        done
    else
        log_info "📟 交互模式: 输入提示词 (Ctrl+C 退出)"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        
        while true; do
            echo -n -e "${BLUE}>> ${NC}"
            if IFS= read -r PROMPT; then
                if [[ -n "$PROMPT" ]]; then
                    execute_prompt "$PROMPT"
                    echo ""
                fi
            else
                break
            fi
        done
    fi
}

trap 'echo ""; log_warn "收到中断信号，退出..."; exit 0' INT TERM

main "$@"
