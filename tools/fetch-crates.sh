#!/usr/bin/env bash
# cargo 的 TLS 握手在本机失败（curl 正常），所以用 curl 把缺失的 .crate 填进缓存。
# 每次 cargo --offline 只会报最先缺的那几个，循环直到不再有新缺失。
set -u
CACHE="$HOME/.cargo/registry/cache/mirrors.tuna.tsinghua.edu.cn-4dc01642fd091eda"
cd "$(dirname "$0")/.." || exit 1
cd src-tauri || exit 1

for round in $(seq 1 60); do
  out=$(CARGO_NET_OFFLINE=true "$HOME/.cargo/bin/cargo.exe" build --offline 2>&1)
  missing=$(echo "$out" | grep -oE 'failed to download `[^`]+`' | sed 's/failed to download `//; s/`$//' | sort -u)
  if [ -z "$missing" ]; then
    echo "$out" | tail -5
    echo "== 没有更多缺失（轮次 $round）=="
    exit 0
  fi
  echo "轮次 $round：缺 $(echo "$missing" | wc -l) 个"
  # 行格式是「name version」，按行取，别用 for 拆（name 里可能有连字符）
  while IFS= read -r spec; do
    [ -z "$spec" ] && continue
    name=$(echo "$spec" | awk '{print $1}')
    ver=$(echo "$spec" | awk "{print \$2}" | sed "s/^v//")
    target="$CACHE/$name-$ver.crate"
    [ -f "$target" ] && continue
    # --ssl-no-revoke：本机 schannel 查吊销列表时连不上 CA，不关掉会随机失败
    code=$(curl -sS --ssl-no-revoke --retry 5 --retry-all-errors -m 60 -o "$target" -w "%{http_code}" \
      "https://static.crates.io/crates/$name/$ver/download")
    if [ "$code" != "200" ]; then
      echo "  失败 $name@$ver HTTP $code"
      rm -f "$target"
    else
      echo "  下载 $name@$ver"
    fi
  done <<< "$missing"
done
echo "轮次用尽"
exit 1
