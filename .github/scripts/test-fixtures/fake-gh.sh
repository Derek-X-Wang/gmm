#!/usr/bin/env bash

set -euo pipefail

case "${FAKE_GH_MODE:-}:$1:$2" in
  existing-issue:label:create)
    exit 0
    ;;
  existing-issue:issue:list)
    echo "Recommended Importer Origin for himi stopped resolving: a/b"
    ;;
  existing-issue:issue:create)
    echo "issue creation must not be reached when the alert is already open" >&2
    exit 99
    ;;
  no-artifact:api:*)
    echo 0
    ;;
  download-failure:api:*)
    echo 1
    ;;
  download-failure:run:download)
    echo "simulated download failure" >&2
    exit 42
    ;;
  *)
    echo "unexpected fake gh call: mode=${FAKE_GH_MODE:-unset} args=$*" >&2
    exit 98
    ;;
esac
