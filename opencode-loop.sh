#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

load_config() {
    local config_file="${SCRIPT_DIR}/opencode-loop.env"
    
    if [[ -f "$config_file" ]]; then
        set -a
        source "$config_file"
        set +a
        return 0
    fi
    return 1
}

DEFAULT_MODEL=""
DEFAULT_FORMAT="default"
DEFAULT_WORKDIR="."
DEFAULT_EXTRA_OPTS=""
DEFAULT_LOG_FILE="/tmp/opencode-loop.log"
DEFAULT_TIMEOUT="300"
DEFAULT_FALLBACK_MODELS="opencode/gpt-5-nano,opencode/trinity-large-preview-free,opencode/claude-3.5-sonnet-free"

load_config

MODEL="${OPENCODE_MODEL:-${MODEL:-$DEFAULT_MODEL}}"
FORMAT="${OPENCODE_FORMAT:-${FORMAT:-$DEFAULT_FORMAT}}"
WORKDIR="${OPENCODE_DIR:-${WORKDIR:-$DEFAULT_WORKDIR}}"
EXTRA_OPTS="${OPENCODE_OPTS:-${EXTRA_OPTS:-$DEFAULT_EXTRA_OPTS}}"
LOG_FILE="${OPENCODE_LOG_FILE:-${LOG_FILE:-$DEFAULT_LOG_FILE}}"
TIMEOUT="${OPENCODE_TIMEOUT:-${TIMEOUT:-$DEFAULT_TIMEOUT}}"
FALLBACK_MODELS="${OPENCODE_FALLBACK:-${FALLBACK_MODELS:-$DEFAULT_FALLBACK_MODELS}}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

CURRENT_MODEL=""
RETRY_COUNT=0
MAX_RETRIES=2

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
    --timeout SEC           执行超时秒数 (默认 300)
    --fallback MODEL        备用模型列表逗号分隔
EOF
}

PROMPT=""
INPUT_FILE=""
CONTINUE_SESSION=""
SESSION_ID=""
DRY_RUN=false
OUTPUT_FILE=""
FORCE_LOOP=false
TIMEOUT_ARG=""
FALLBACK_ARG=""

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
        --timeout)
            TIMEOUT_ARG="$2"
            shift 2
            ;;
        --fallback)
            FALLBACK_ARG="$2"
            shift 2
            ;;
        *)
            PROMPT="$1"
            shift
            ;;
    esac
done

[[ -n "$TIMEOUT_ARG" ]] && TIMEOUT="$TIMEOUT_ARG"
[[ -n "$FALLBACK_ARG" ]] && FALLBACK_MODELS="$FALLBACK_ARG"
CURRENT_MODEL="$MODEL"

build_cmd() {
    local prompt="$1"
    local model="$2"
    local cmd="opencode run"
    cmd="$cmd \"$prompt\""
    [[ -n "$model" ]] && cmd="$cmd --model $model"
    cmd="$cmd --format $FORMAT"
    [[ -n "$EXTRA_OPTS" ]] && cmd="$cmd $EXTRA_OPTS"
    [[ -n "$CONTINUE_SESSION" ]] && cmd="$cmd $CONTINUE_SESSION"
    [[ -n "$SESSION_ID" ]] && cmd="$cmd $SESSION_ID"
    cmd="$cmd --dir $WORKDIR"
    echo "$cmd"
}

check_error() {
    local output="$1"

    local strong_pattern="401|429|403|quota|token.*(invalid|expired|insufficient|exceed)|invalid key|expired key|authentication.*fail|unauthoriz|rate.*limit|Request rate increased too quickly|api.*key"
    local matched
    matched=$(echo "$output" | grep -iE "$strong_pattern" -m 1 || true)

    if [[ -n "$matched" ]]; then
        echo "$matched"
        return 0
    fi
    return 1
}

get_next_model() {
    local current="$1"
    local found=false
    
    IFS=',' read -ra MODELS <<< "$FALLBACK_MODELS"
    for m in "${MODELS[@]}"; do
        m=$(echo "$m" | xargs)
        [[ -z "$m" ]] && continue
        if [[ "$found" == "true" ]]; then
            echo "$m"
            return 0
        fi
        if [[ "$m" == "$current" ]]; then
            found=true
        fi
    done
    
    echo ""
    return 1
}

execute_prompt() {
    local prompt="$1"
    local model="$2"
    local cmd
    cmd=$(build_cmd "$prompt" "$model")
    
    log_step "执行 [$model]: $prompt"
    
    if [[ "$DRY_RUN" == "true" ]]; then
        log_info "[DRY-RUN] $cmd"
        return 0
    fi
    
    log_info "命令: $cmd"
    log_info "⏳ 执行中..."
    
    local exit_code=0
    local output
    
    output=$($cmd 2>&1) || exit_code=$?

    echo "$output" | tee -a "$LOG_FILE"

    local matched
    matched=$(check_error "$output" || true)
    if [[ -n "$matched" ]]; then
        log_warn "检测到可能的 API 警告匹配: $matched"
        if [[ $exit_code -ne 0 ]]; then
            log_error "❌ 检测到 API 错误 (rate limit/quota/invalid)"
            return 2
        fi
    fi
    
    if [[ $exit_code -eq 0 ]]; then
        log_info "✅ 执行成功"
    else
        log_error "❌ 执行失败 (exit code: $exit_code)"
    fi
    
    return $exit_code
}

run_with_fallback() {
    local prompt="$1"
    local model="$2"
    local attempt=1
    local model_count
    local retry_attempt=0
    local max_retries=3
    model_count=$(echo "$FALLBACK_MODELS" | awk -F',' '{print NF}')
    local max_attempts=$((model_count + 1))
    
    while true; do
        log_info "=== 尝试 $attempt/$max_attempts (模型: $model) ==="
        
        local result
        execute_prompt "$prompt" "$model"
        result=$?
        
        if [[ $result -eq 0 ]]; then
            return 0
        fi
        
        if [[ $result -eq 2 ]]; then
            if [[ $retry_attempt -lt $max_retries ]]; then
                local wait_time=$(( (2 ** retry_attempt) * 5 ))
                log_warn "🔄 检测到限流，等待 ${wait_time}s 后重试 (${retry_attempt}/${max_retries})"
                sleep $wait_time
                retry_attempt=$((retry_attempt + 1))
                continue
            fi
            
            local next_model
            next_model=$(get_next_model "$model")
            
            if [[ -n "$next_model" ]]; then
                log_warn "🔄 切换模型: $model -> $next_model"
                model="$next_model"
                CURRENT_MODEL="$model"
                retry_attempt=0
            else
                log_error "没有更多备用模型"
                return 1
            fi
        fi
        
        attempt=$((attempt + 1))
        
        if [[ $attempt -gt $max_attempts ]]; then
            log_error "已达到最大尝试次数"
            return 1
        fi
        
        sleep 2
    done
}

main() {
    log_info "🚀 OpenCode Loop 启动"
    log_info "工作目录: $WORKDIR"
    log_info "输出格式: $FORMAT"
    log_info "超时: ${TIMEOUT}s"
    [[ -n "$CURRENT_MODEL" ]] && log_info "当前模型: $CURRENT_MODEL"
    [[ -n "$FALLBACK_MODELS" ]] && log_info "备用模型: $FALLBACK_MODELS"
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
            run_with_fallback "$line" "$CURRENT_MODEL"
            echo ""
            sleep 1
        done < "$INPUT_FILE"
        log_info "📄 文件处理完成，共 $line_num 行"
        return
    fi
    
    if [[ "$FORCE_LOOP" == "true" && -n "$PROMPT" ]]; then
        log_info "🔄 无限循环模式: 自动重复执行 (Ctrl+C 退出)"
        log_info "备用模型: $FALLBACK_MODELS"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        
        while true; do
            echo -e "${BLUE}▶ 执行: $PROMPT${NC}"
            run_with_fallback "$PROMPT" "$CURRENT_MODEL"
            echo ""
            log_info "⏳ 等待 30s 后继续 (防止限流)..."
            sleep 30
        done
        log_info "👋 退出循环"
    elif [[ -n "$PROMPT" || "$FORCE_LOOP" == "true" ]]; then
        if [[ -n "$PROMPT" ]]; then
            run_with_fallback "$PROMPT" "$CURRENT_MODEL"
            echo ""
        fi
        log_info "🔄 无限循环模式: 输入提示词继续 (Ctrl+C 退出)"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        
        while true; do
            echo -n -e "${BLUE}>> ${NC}"
            if IFS= read -r PROMPT; then
                if [[ -n "$PROMPT" ]]; then
                    run_with_fallback "$PROMPT" "$CURRENT_MODEL"
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
                    run_with_fallback "$PROMPT" "$CURRENT_MODEL"
                    echo ""
                fi
            else
                break
            fi
        done
    fi
}

trap 'echo ""; log_warn "收到中断信号，退出..."; exit 0' INT TERM

cleanup() {
    pkill -P $$ 2>/dev/null
    rm -f /tmp/opencode_output_$$ /tmp/opencode_error_$$
}

trap cleanup EXIT

main "$@"
