#!/bin/bash
# metering.sh — MERIT discipline: token/dollar counting helpers for run.sh.
# Source this file; do not execute directly.
# Counting rule (harness spec 4.5): chars/4 ceiling per question. Price per
# 1K tokens default 0.002; override by exporting PRICE_PER_1K.

PRICE_PER_1K="${PRICE_PER_1K:-0.002}"

# tokens_for_text <text>: ceiling(chars/4) via char count (multibyte-aware).
tokens_for_text() {
    local chars
    chars=$(printf '%s' "$1" | wc -m)
    echo $(( (chars + 3) / 4 ))
}

# dollars_for_tokens <tokens>: tokens * PRICE_PER_1K / 1000, 6 decimals.
dollars_for_tokens() {
    local tokens="$1"
    awk -v t="$tokens" -v p="$PRICE_PER_1K" 'BEGIN { printf "%.6f", t * p / 1000 }'
}
