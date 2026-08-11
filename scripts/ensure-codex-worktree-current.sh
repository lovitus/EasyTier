#!/usr/bin/env bash

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/ensure-codex-worktree-current.sh [--check|--update]

  --check   Fetch release refs and report whether this worktree is current.
  --update  Additionally fast-forward a clean codex/current worktree (default).

Feature and release worktrees are never modified automatically.
EOF
}

mode="${1:---update}"
case "$mode" in
    --check | --update) ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac

repo_root=$(git rev-parse --show-toplevel 2>/dev/null) || {
    echo "worktree guard: not inside a Git worktree" >&2
    exit 1
}
cd "$repo_root"

branch=$(git symbolic-ref --quiet --short HEAD) || {
    echo "worktree guard: detached HEAD is not a valid development base" >&2
    exit 1
}

dirty=$(git status --porcelain --untracked-files=normal)
if [[ -n "$dirty" ]]; then
    echo "worktree guard: refusing to select a base from a dirty worktree ($branch)" >&2
    printf '%s\n' "$dirty" >&2
    exit 1
fi

# codex/current is the sole accepted integration pointer. Release and evidence
# branches advance it only after validation, so tags and version-named worktrees
# are immutable evidence rather than competing definitions of "current".
remote_current_ref="refs/heads/codex/current"
if [[ -z "$(git ls-remote --heads origin "$remote_current_ref")" ]]; then
    echo "worktree guard: origin/codex/current is unavailable" >&2
    exit 1
fi
git fetch --quiet origin "+${remote_current_ref}:refs/remotes/origin/codex/current"
target_ref="refs/remotes/origin/codex/current"

head_sha=$(git rev-parse HEAD)
target_sha=$(git rev-parse "$target_ref^{commit}")

if [[ "$head_sha" == "$target_sha" ]]; then
    echo "worktree guard: current ($branch@$head_sha, target $target_ref)"
    exit 0
fi

if git merge-base --is-ancestor "$target_sha" "$head_sha"; then
    echo "worktree guard: current branch already contains $target_ref ($target_sha)"
    exit 0
fi

if git merge-base --is-ancestor "$head_sha" "$target_sha"; then
    if [[ "$branch" == "codex/current" && "$mode" == "--update" ]]; then
        git merge --ff-only "$target_sha"
        echo "worktree guard: fast-forwarded codex/current to $target_ref ($target_sha)"
        exit 0
    fi
    echo "worktree guard: stale worktree $branch@$head_sha; accepted target is $target_ref@$target_sha" >&2
    if [[ "$branch" == "codex/current" ]]; then
        echo "rerun with --update to fast-forward the canonical worktree" >&2
    else
        echo "create new work from codex/current or deliberately rebase/cherry-pick this branch" >&2
    fi
    exit 1
fi

echo "worktree guard: $branch@$head_sha diverges from accepted target $target_ref@$target_sha" >&2
echo "resolve the branch explicitly; automatic reset/rebase is forbidden" >&2
exit 1
