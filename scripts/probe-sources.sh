#!/bin/bash
# read-only probe: same artifact from several mirrors, concurrently; report who wins.
set -u
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

probe() {
  local tag="$1" url="$2" out="$TMP/$(echo "$tag$url" | md5sum | cut -c1-12)"
  curl -sL --max-time 60 -o /dev/null \
    -w "%{http_code} %{size_download} %{speed_download} %{time_total}" "$url" > "$out"
  echo "$tag|$(cat "$out")" >> "$TMP/result"
}

race() {
  local path="$1"
  : > "$TMP/result"
  local central="https://repo.maven.apache.org/maven2/$path"
  local aliyun="https://maven.aliyun.com/repository/public/$path"
  local bmcl="https://bmclapi2.bangbang93.com/maven/$path"
  probe central "$central" &
  probe aliyun "$aliyun" &
  probe bmcl "$bmcl" &
  wait
  echo "--- $path"
  sort -t'|' -k2 "$TMP/result" | while IFS='|' read -r tag stat; do
    read -r code size speed time <<< "$stat"
    printf "  %-8s code=%s size=%8s speed=%10.0f B/s time=%ss\n" \
      "$tag" "$code" "$size" "$speed" "$time"
  done
}

for p in \
  "org/ow2/asm/asm/9.10.1/asm-9.10.1.jar" \
  "com/google/guava/guava/33.6.0-jre/guava-33.6.0-jre.jar" \
  "io/netty/netty-all/4.2.16.Final/netty-all-4.2.16.Final.jar" \
  "org/apache/logging/log4j/log4j-core/2.26.0/log4j-core-2.26.0.jar" \
  "org/lwjgl/lwjgl/3.4.3/lwjgl-3.4.3.jar" \
  "org/jline/jline/4.1.3/jline-4.1.3.jar" ; do
  race "$p"
done
