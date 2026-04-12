#!/bin/bash
set -e
set -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

load_config() {
    local config_file="${SCRIPT_DIR}/gemini-loop.env"

    if [[ -f "$config_file" ]]; then
        set -a
        # shellcheck source=/dev/null
        source "$config_file"
        set +a
        return 0
    fi
    return 1
}

DEFAULT_MODEL=""
DEFAULT_WORKDIR="."
DEFAULT_EXTRA_OPTS=""
DEFAULT_LOG_FILE="/tmp/gemini-loop.log"
DEFAULT_TIMEOUT="300"
DEFAULT_FALLBACK_MODELS="gemini-2.5-flash,gemini-2.0-flash"

load_config

MODEL="${GEMINI_MODEL:-${MODEL:-$DEFAULT_MODEL}}"
WORKDIR="${GEMINI_DIR:-${WORKDIR:-$DEFAULT_WORKDIR}}"
EXTRA_OPTS="${GEMINI_OPTS:-${EXTRA_OPTS:-$DEFAULT_EXTRA_OPTS}}"
LOG_FILE="${GEMINI_LOG_FILE:-${LOG_FILE:-$DEFAULT_LOG_FILE}}"
TIMEOUT="${GEMINI_TIMEOUT:-${TIMEOUT:-$DEFAULT_TIMEOUT}}"
FALLBACK_MODELS="${GEMINI_FALLBACK:-${FALLBACK_MODELS:-$DEFAULT_FALLBACK_MODELS}}"
GEMINI_AUTO_CONFIRM="${GEMINI_AUTO_CONFIRM:-true}"
GEMINI_SESSION_FLAG="${GEMINI_SESSION_FLAG:---resume}"
GEMINI_APPROVAL_MODE="${GEMINI_APPROVAL_MODE:-yolo}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

CURRENT_MODEL=""
CURRENT_CMD_PID=""
CURRENT_OUTPUT_FILE=""

log() {
    local level="$1"
    local msg="$2"
    local timestamp
    timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo -e "[$timestamp] [$level] $msg" | tee -a "$LOG_FILE"
}

log_info()  { log "INFO" "${GREEN}$1${NC}"; }
log_warn()  { log "WARN" "${YELLOW}$1${NC}"; }
log_error() { log "ERROR" "${RED}$1${NC}"; }
log_step()  { log "STEP" "${BLUE}$1${NC}"; }

show_help() {
    cat << 'EOF'
Usage: gemini-loop.sh [OPTIONS] [PROMPT]

Options:
    -h, --help              Show help
    -l, --loop              无限循环模式 (执行后继续等待输入)
    -m, --model MODEL       Model (e.g., gemini-2.5-pro)
    -f, --file FILE         Read prompts from file (one per line, then exit)
    -s, --session ID        Specify a session ID for Gemini CLI (passed as --session ID)
    -d, --dir DIR           Working directory
    -o, --output FILE       Output file
    --no-exec               Dry-run mode
    --timeout SEC           执行超时秒数 (默认 300)
    --fallback MODELS       备用模型列表（逗号分隔）
EOF
}

PROMPT=""
INPUT_FILE=""
SESSION_ID=""
SESSION_ARG=""
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
        -s|--session)
            SESSION_ID="$2"
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

    # Gemini CLI 常用模式：gemini -p "..." -m model
    local cmd="gemini"
    [[ -n "$prompt" ]] && cmd="$cmd -p \"$prompt\""
    [[ -n "$model" ]] && cmd="$cmd -m $model"

    # Prefer explicit approval mode when supported; yolo removes confirmation prompts.
    if [[ "$GEMINI_AUTO_CONFIRM" == "true" && -n "$GEMINI_APPROVAL_MODE" ]]; then
        cmd="$cmd --approval-mode $GEMINI_APPROVAL_MODE"
    fi

    [[ -n "$EXTRA_OPTS" ]] && cmd="$cmd $EXTRA_OPTS"

    # If a session arg was provided, pass it through
    if [[ -n "$SESSION_ARG" ]]; then
        cmd="$cmd $SESSION_ARG"
    fi

    echo "$cmd"
}

check_error() {
    local output="$1"

    if echo "$output" | grep -qiE "quota|token.*(limit|exceed|insufficient)|credits?|rate.*limit|api.*(error|key).*(invalid|expired)|authentication.*fail|unauthorized|401|429|403|Request rate increased too quickly|resource exhausted"; then
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
        if [[ -z "$current" ]]; then
            echo "$m"
            return 0
        fi
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

# Detect whether the configured GEMINI_SESSION_FLAG is supported by the
# installed gemini CLI. If supported, set SESSION_ARG accordingly.
detect_session_flag_support() {
    SESSION_ARG=""
    if [[ -z "$SESSION_ID" ]]; then
        return 0
    fi

    # If user hasn't specified a session flag in env, do not pass a session.
    if [[ -z "$GEMINI_SESSION_FLAG" ]]; then
        log_warn "GEMINI_SESSION_FLAG 未设置，跳过传递 session ID"
        return 0
    fi

    if ! command -v gemini >/dev/null 2>&1; then
        return 0
    fi

    local help_output
    help_output=$(gemini --help 2>&1 || true)
    # Use fixed string match for the configured flag (supports short or long forms)
    if echo "$help_output" | grep -Fq -- "$GEMINI_SESSION_FLAG"; then
        SESSION_ARG="$GEMINI_SESSION_FLAG $SESSION_ID"
        return 0
    else
        log_warn "已配置的 session 标志 '$GEMINI_SESSION_FLAG' 在本地 gemini CLI 中未被识别，已跳过传递 session 参数"
        SESSION_ARG=""
        return 1
    fi
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
    local output_file
    local output
    output_file=$(mktemp /tmp/gemini-loop-output.XXXXXX)
    CURRENT_OUTPUT_FILE="$output_file"

    coproc GEMINI_PROC {
        cd "$WORKDIR" || exit 1
        if command -v timeout >/dev/null 2>&1; then
            if command -v setsid >/dev/null 2>&1; then
                setsid timeout "$TIMEOUT" bash -lc "$cmd" 2>&1
            else
                timeout "$TIMEOUT" bash -lc "$cmd" 2>&1
            fi
        else
            if command -v setsid >/dev/null 2>&1; then
                setsid bash -lc "$cmd" 2>&1
            else
                bash -lc "$cmd" 2>&1
            fi
        fi
    }
    CURRENT_CMD_PID="$COPROC_PID"

    while IFS= read -r line <&"${GEMINI_PROC[0]}" || [[ -n "$line" ]]; do
        printf '%s\n' "$line"
        printf '%s\n' "$line" >> "$LOG_FILE"
        printf '%s\n' "$line" >> "$output_file"
        if [[ -n "$OUTPUT_FILE" ]]; then
            printf '%s\n' "$line" >> "$OUTPUT_FILE"
        fi
    done

    if wait "$CURRENT_CMD_PID"; then
        exit_code=0
    else
        exit_code=$?
    fi

    output=$(cat "$output_file" 2>/dev/null || true)
    rm -f "$output_file"
    CURRENT_OUTPUT_FILE=""
    CURRENT_CMD_PID=""

    if check_error "$output"; then
        log_error "❌ 检测到 API 错误 (rate limit/quota/invalid)"
        return 2
    fi

    if [[ $exit_code -eq 0 ]]; then
        log_info "✅ 执行成功"
    elif [[ $exit_code -eq 124 ]]; then
        log_error "❌ 执行超时 (${TIMEOUT}s)"
    else
        log_error "❌ 执行失败 (exit code: $exit_code)"
    fi

    return $exit_code
}

run_with_fallback() {
    local prompt="$1"
    local model="$2"
    local attempt=1
    local retry_attempt=0
    local max_retries=3
    local model_count

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
                sleep "$wait_time"
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
    if ! command -v gemini >/dev/null 2>&1; then
        log_error "未找到 gemini 命令，请先安装 Gemini CLI"
        exit 127
    fi

    if [[ "$GEMINI_AUTO_CONFIRM" == "true" ]]; then
        log_info "自动确认模式: --approval-mode $GEMINI_APPROVAL_MODE"
    fi

    # Detect whether configured session flag is supported and build SESSION_ARG
    detect_session_flag_support

    log_info "🚀 Gemini Loop 启动"
    log_info "工作目录: $WORKDIR"
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
        local line_num=0
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

    if [[ "$FORCE_LOOP" == "true" ]]; then
        if [[ -z "$PROMPT" ]]; then
            log_error "无限循环模式需要先提供一个初始提示词"
            exit 1
        fi

        log_info "🔄 无限循环模式: 自动重复执行同一提示词 (Ctrl+C 退出)"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

        while true; do
            echo -e "${BLUE}▶ 执行: $PROMPT${NC}"
            run_with_fallback "$PROMPT" "$CURRENT_MODEL"
            echo ""
            log_info "⏳ 等待 30s 后继续 (防止限流)..."
            sleep 30
        done
    elif [[ -n "$PROMPT" ]]; then
        run_with_fallback "$PROMPT" "$CURRENT_MODEL"
        echo ""
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

cleanup_runtime() {
    if [[ -n "$CURRENT_CMD_PID" ]]; then
        kill -TERM -- "-$CURRENT_CMD_PID" 2>/dev/null || true
        kill -TERM "$CURRENT_CMD_PID" 2>/dev/null || true
    fi
    [[ -n "$CURRENT_OUTPUT_FILE" && -f "$CURRENT_OUTPUT_FILE" ]] && rm -f "$CURRENT_OUTPUT_FILE" 2>/dev/null || true
    pkill -P $$ 2>/dev/null || true
}

on_interrupt() {
    echo ""
    log_warn "收到中断信号，退出..."
    cleanup_runtime
    exit 130
}

trap on_interrupt INT TERM

cleanup() {
    cleanup_runtime
}

trap cleanup EXIT

main "$@"
