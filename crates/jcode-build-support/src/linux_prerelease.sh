# Embedded by the build hook. Send only an immutable SHA to the canonical repo.
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
exec gh workflow run selfdev-linux.yml --repo 1jehuang/jcode --ref master -f "sha=$sha"
