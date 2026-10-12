#!/bin/bash
# Deliberately hostile real worker. Uses only staged bash, sleep and setsid.
set -u
mode=$1
outside=$2
case "$mode" in
  escape)
    printf 'allowed\n' > allowed
    (printf 'escape\n' > "$outside/absolute-write") 2>/dev/null || :
    (printf 'escape\n' > escape-link/symlink-write) 2>/dev/null || :
    (printf 'escape\n' > escape-link/missing/tail-write) 2>/dev/null || :
    if [[ -r "$outside/sentinel" ]]; then printf 'LEAK\n' > sentinel-result; else printf 'absent\n' > sentinel-result; fi
    ;;
  home)
    if [[ "$HOME" = /home/worker && "$XDG_RUNTIME_DIR" = /run/worker && ! -e /run/user && ! -e /home/mason && ! -e /root && ! -e /run/dbus && ! -e /run/ironbark-core ]]; then
      printf 'private\n' > home-result
    else printf 'LEAK\n' > home-result; fi
    ;;
  fds)
    if [[ ! -e /proc/self/fd/3 && ! -e /proc/self/fd/4 && ! -e /proc/self/fd/5 && ! -v LD_PRELOAD && ! -v BASH_ENV && ! -v ALPHACODE_HOME && ! -v HTTP_PROXY && ! -v HTTPS_PROXY ]]; then
      printf 'closed\n' > fd-result
    else printf 'LEAK\n' > fd-result; fi
    ;;
  network)
    if (exec 9<>"/dev/tcp/127.0.0.1/$3") 2>/dev/null; then printf 'BYPASS\n' > network-result; else printf 'denied\n' > network-result; fi
    ;;
  children)
    trap '' INT TERM
    /usr/bin/sleep 600 &
    /bin/bash -c 'trap "" INT TERM; /usr/bin/sleep 600 & wait' &
    # setsid and shell exit detach a double-forked process from the tool group.
    /usr/bin/setsid /bin/bash -c '/bin/bash -c "trap \"\" INT TERM; exec /usr/bin/sleep 600" </dev/null >/dev/null 2>&1 & exit 0' &
    /usr/bin/sleep 0.1
    printf 'ready\n' > children-ready
    wait
    ;;
  substitute)
    # Hostile runtime-ancestor attacks. The approved workspace bind stays
    # writable; /, /home, and the private HOME boundary must stay read-only.
    (printf 'escape\n' > /root-escape) 2>/dev/null || :
    (cd / && mkdir sibling-home) 2>/dev/null || :
    (cd /home && mv worker worker-renamed) 2>/dev/null || :
    printf 'ok\n' > private-write
    if [[ -e /home/worker && ! -e /home/sibling-home && ! -e /home/worker-renamed && ! -e /root-escape ]]; then
      printf 'private\n' > home-result
    else
      { echo "HOME=[$HOME] XDG=[$XDG_RUNTIME_DIR] cwd=$(pwd)";
        echo "-- mountinfo targets:";
        while read -r _ _ _ _ target rest; do echo "  $target | $rest"; done < /proc/self/mountinfo; } > home-diag
      printf 'LEAK\n' > home-result; fi
    ;;
  *) exit 64 ;;
esac
