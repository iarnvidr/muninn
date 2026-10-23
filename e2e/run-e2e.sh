#!/usr/bin/env bash
# Host-side runner for the live check: drives scenario.py inside the test container,
# cuts the container's network for the offline replay, and copies the screenshots out.
#   run-e2e.sh CONTAINER [OUT_DIR]
# The container is the Linux test image (muninn-linux-test) with the sources synced to
# /build and the debug build at /cache/target/debug/muninn (see README, "Development").
set -uo pipefail
c="${1:-mu-dev}"
out="${2:-/tmp/muninn-e2e}"
mkdir -p "$out"
x() { docker exec "$c" bash -c "cd /build/e2e && $*"; }
rc=0
x "./run-app.sh >/dev/null && sleep 6 && python3 scenario.py online" || rc=1
vid="$(x "python3 -c \"import json;r=json.load(open('/tmp/shots/results-online.json'));print(r.get('video_id',''))\"")"
variant="$(x "python3 -c \"import json;r=json.load(open('/tmp/shots/results-online.json'));print(r.get('variant',''))\"")"
if [ -n "$vid" ]; then
  net="$(docker inspect "$c" --format '{{range $k,$v := .NetworkSettings.Networks}}{{$k}} {{end}}' | awk '{print $1}')"
  docker network disconnect "$net" "$c"
  x "python3 scenario.py offline $vid $variant" || rc=1
  docker network connect "$net" "$c"
fi
x "KEEP_HOME=1 ./run-app.sh >/dev/null && sleep 8 && python3 scenario.py restart" || rc=1
x "python3 scenario.py popup" || rc=1
x "./run-app.sh >/dev/null && sleep 6 && python3 scenario.py ram" || rc=1
docker cp "$c":/tmp/shots/. "$out"/
exit $rc
