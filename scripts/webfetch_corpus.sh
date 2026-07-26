#!/usr/bin/env bash
# Fetch a diverse corpus of real pages for webfetch extraction testing.
#
# The corpus intentionally spans different generators and page shapes so that
# extraction rules are validated for generality rather than tuned to one site:
#   - wiki (MediaWiki/Parsoid)      - heavy nav, JSON-in-attributes
#   - api docs (rustdoc, mdBook)    - encoded playground links, deep nesting
#   - news/blog article             - <article>/<header> semantics
#   - forum/discussion thread       - repeated post chrome
#   - standards/spec document       - very long, table-of-contents heavy
#   - SPA/JS-heavy landing page     - little server-rendered prose
#   - plain text / markdown / JSON  - non-HTML passthrough paths
#
# Usage: scripts/webfetch_corpus.sh [outdir]
set -uo pipefail

OUT="${1:-${JCODE_SCRATCH_DIR:-/tmp}/webfetch-corpus}"
mkdir -p "$OUT"
UA="Mozilla/5.0 (compatible; JCode/1.0)"

# name|url
CORPUS=$(cat <<'EOF'
wikipedia_rust|https://en.wikipedia.org/wiki/Rust_(programming_language)
wikipedia_short|https://en.wikipedia.org/wiki/Ferris_wheel
rustdoc_vec|https://doc.rust-lang.org/std/vec/struct.Vec.html
rustdoc_index|https://doc.rust-lang.org/std/index.html
mdbook_book|https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html
mdn_fetch|https://developer.mozilla.org/en-US/docs/Web/API/Fetch_API/Using_Fetch
python_docs|https://docs.python.org/3/library/json.html
rfc_http|https://www.rfc-editor.org/rfc/rfc9110.html
whatwg_spec|https://html.spec.whatwg.org/multipage/introduction.html
blog_article|https://blog.rust-lang.org/2024/09/05/Rust-1.81.0.html
news_bbc|https://www.bbc.com/news
hn_thread|https://news.ycombinator.com/item?id=1
so_question|https://stackoverflow.com/questions/11227809/why-is-processing-a-sorted-array-faster-than-processing-an-unsorted-array
github_readme_html|https://github.com/rust-lang/rust
gh_raw_markdown|https://raw.githubusercontent.com/rust-lang/rust/master/README.md
gh_api_json|https://api.github.com/repos/rust-lang/rust
plain_text|https://www.rfc-editor.org/rfc/rfc2616.txt
spa_landing|https://vercel.com
arxiv_abs|https://arxiv.org/abs/1706.03762
EOF
)

echo "name|status|content_type|bytes"
while IFS='|' read -r name url; do
  [ -z "$name" ] && continue
  hdr="$OUT/$name.headers"
  body="$OUT/$name.html"
  code=$(curl -sL -A "$UA" --max-time 30 -D "$hdr" -o "$body" -w '%{http_code}' "$url" 2>/dev/null)
  ctype=$(grep -i '^content-type:' "$hdr" 2>/dev/null | tail -1 | tr -d '\r' | cut -d' ' -f2- )
  size=$(wc -c < "$body" 2>/dev/null || echo 0)
  printf '%s|%s|%s|%s\n' "$name" "$code" "${ctype:-unknown}" "$size"
  echo "$url" > "$OUT/$name.url"
done <<< "$CORPUS"
