#!/usr/bin/env bash
set -euo pipefail

# 用法:
#   ./merge4gemini.sh                       # 自动识别主线分支，匹配 gemini-*
#   ./merge4gemini.sh master "gemini-*"
#   DRY_RUN=1 ./merge4gemini.sh             # 仅预览，不实际合并
#
# 环境变量:
#   REMOTE=origin                     # 远端名，默认 origin

TARGET_BRANCH="${1:-}"
PATTERN="${2:-gemini-*}"
REMOTE="${REMOTE:-origin}"
DRY_RUN="${DRY_RUN:-0}"

resolve_default_branch() {
  local b

  # 1) 优先用远端 HEAD 指向分支
  b="$(git symbolic-ref --quiet --short "refs/remotes/${REMOTE}/HEAD" 2>/dev/null || true)"
  if [[ -n "${b}" ]]; then
    echo "${b#${REMOTE}/}"
    return
  fi

  # 2) 常见主线名
  for b in main master; do
    if git show-ref --verify --quiet "refs/heads/${b}" || git show-ref --verify --quiet "refs/remotes/${REMOTE}/${b}"; then
      echo "${b}"
      return
    fi
  done

  # 3) 回退到当前分支
  git rev-parse --abbrev-ref HEAD
}

ensure_local_branch() {
  local b="$1"
  if git show-ref --verify --quiet "refs/heads/${b}"; then
    return
  fi
  if git show-ref --verify --quiet "refs/remotes/${REMOTE}/${b}"; then
    git checkout -b "${b}" "${REMOTE}/${b}"
    return
  fi
  echo "Target branch not found locally/remotely: ${b}"
  exit 1
}

branch_name_from_ref() {
  local ref="$1"
  local name="${ref#refs/heads/}"
  name="${name#refs/remotes/${REMOTE}/}"
  echo "${name}"
}

match_branch_pattern() {
  local name="$1"
  # 既支持 gemini-*，也支持 vibe/gemini-* 这种前缀路径
  [[ "${name}" == ${PATTERN} || "${name##*/}" == ${PATTERN} ]]
}

if [[ -z "${TARGET_BRANCH}" ]]; then
  TARGET_BRANCH="$(resolve_default_branch)"
fi

echo "==> target: ${TARGET_BRANCH}"
echo "==> pattern: ${PATTERN}"
echo "==> remote: ${REMOTE}"
echo "==> dry-run: ${DRY_RUN}"

git rev-parse --is-inside-work-tree >/dev/null

echo "==> fetch latest refs"
git fetch --all --prune

echo "==> checkout target branch"
ensure_local_branch "${TARGET_BRANCH}"
git checkout "${TARGET_BRANCH}"
if git show-ref --verify --quiet "refs/remotes/${REMOTE}/${TARGET_BRANCH}"; then
  git pull --ff-only "${REMOTE}" "${TARGET_BRANCH}"
else
  echo "==> no remote tracking branch ${REMOTE}/${TARGET_BRANCH}, skip pull"
fi

BACKUP="backup/${TARGET_BRANCH}-before-$(date +%Y%m%d-%H%M%S)"
echo "==> create backup branch: ${BACKUP}"
git branch "${BACKUP}"

echo "==> enable rerere"
git config rerere.enabled true

# 收集候选 ref（本地 + 远端），并按 PATTERN 过滤
mapfile -t ALL_REFS < <(
  {
    git for-each-ref --format='%(refname)' "refs/heads"
    git for-each-ref --format='%(refname)' "refs/remotes/${REMOTE}"
  } | sort -u
)

CANDIDATE_REFS=()
for ref in "${ALL_REFS[@]}"; do
  # 跳过远端 HEAD 指针
  if [[ "${ref}" == "refs/remotes/${REMOTE}/HEAD" ]]; then
    continue
  fi

  short="$(branch_name_from_ref "${ref}")"
  if match_branch_pattern "${short}"; then
    CANDIDATE_REFS+=("${ref}")
  fi
done

if [[ ${#CANDIDATE_REFS[@]} -eq 0 ]]; then
  echo "No branches matched pattern: ${PATTERN}"
  exit 0
fi

echo "==> matched refs:"
for ref in "${CANDIDATE_REFS[@]}"; do
  echo "  - ${ref}"
done

for ref in "${CANDIDATE_REFS[@]}"; do
  # 跳过目标分支自身（本地/远端两种写法）
  if [[ "${ref}" == "refs/heads/${TARGET_BRANCH}" || "${ref}" == "refs/remotes/${REMOTE}/${TARGET_BRANCH}" ]]; then
    continue
  fi

  short="$(branch_name_from_ref "${ref}")"

  # 已包含在当前主线则跳过
  if git merge-base --is-ancestor "${ref}" HEAD; then
    echo ">>> skip (already merged): ${short}"
    continue
  fi

  echo ">>> merging: ${short}"
  if [[ "${DRY_RUN}" == "1" ]]; then
    echo "[DRY] git merge --no-ff ${ref} -m \"merge: ${short} into ${TARGET_BRANCH}\""
    continue
  fi

  if ! git merge --no-ff "${ref}" -m "merge: ${short} into ${TARGET_BRANCH}"; then
    echo
    echo "!!! Merge conflict on: ${short}"
    echo "Resolve conflicts, then run: git add -A && git merge --continue"
    echo "Or abort: git merge --abort"
    echo "Rollback to backup: git reset --hard ${BACKUP}"
    exit 2
  fi
done

echo
echo "✅ All done."
echo "Next: run tests, then push:"
echo "   cargo test"
echo "   git push ${REMOTE} ${TARGET_BRANCH}"
echo "Backup branch: ${BACKUP}"