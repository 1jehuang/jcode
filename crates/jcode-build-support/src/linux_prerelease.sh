# Embedded by the build hook. Only send the SHA and explicitly supplied public highlights.
set -eu
sha=$1
eligible() {
    [ "$(git config --bool jcode.linuxPrerelease)" = true ] &&
    [ "$(git symbolic-ref -q HEAD)" = refs/heads/master ] &&
    [ "$(git rev-parse HEAD)" = "$sha" ] &&
    status=$(git status --porcelain=v1 --untracked-files=all --ignore-submodules=none) &&
    [ -z "$status" ]
}
eligible || exit 0
remote=$(gh api --hostname github.com repos/1jehuang/jcode/git/ref/heads/master --jq .object.sha)
[ "$remote" = "$sha" ] || exit 0
eligible || exit 0
notes=${2-}
set -- -f "sha=$sha"
if [ -n "$notes" ]; then set -- "$@" -f "release_notes=$notes"; fi
exec gh workflow run selfdev-linux.yml --repo 1jehuang/jcode --ref master "$@"
