#!/bin/bash
# 用法:
#   37上: bash /tmp/easy_bench.sh a [stealth] [secure]
#   38上: bash /tmp/easy_bench.sh b [stealth] [secure]
# 等a先跑好,看到 READY 后跑 b

ROLE=$1
EASY=/tmp/easytier-core

cleanup() {
  pkill -9 easytier-core 2>/dev/null
  pkill -9 iperf3 2>/dev/null
  ip link del tun0 2>/dev/null
  sleep 1
}

build_flags() {
  local FLAGS=""
  for f in "$@"; do
    case "$f" in
      stealth) FLAGS="$FLAGS --stealth-mode" ;;
      secure)  FLAGS="$FLAGS --secure-mode" ;;
    esac
  done
  echo "$FLAGS"
}

FLAGS=$(build_flags "$@")

if [ "$ROLE" = "a" ]; then
  cleanup
  echo "=== EasyTier A 启动 $FLAGS ==="
  setsid $EASY --instance-name a --network-name t99 --network-secret t99 \
    -i 10.200.1.1 -l tcp://0.0.0.0:21099 $FLAGS \
    > /tmp/ea.log 2>&1 < /dev/null &
  sleep 4
  if ip addr show tun0 2>/dev/null | grep -q "inet 10.200.1.1"; then
    echo "READY - A OK"
    pkill -9 iperf3 2>/dev/null
    sleep 1
    iperf3 -s -p 19999 -D 2>/dev/null
    ip route add 10.200.2.0/24 dev tun0 2>/dev/null
    echo "iperf3 server started (route added)"
  else
    echo "FAIL - A TUN not ready, check /tmp/ea.log"
  fi

elif [ "$ROLE" = "b" ]; then
  cleanup
  echo "=== EasyTier B 启动 $FLAGS ==="
  setsid $EASY --instance-name b --network-name t99 --network-secret t99 \
    -i 10.200.2.1 -l tcp://0.0.0.0:21098 -p tcp://192.168.1.37:21099 \
    $FLAGS > /tmp/eb.log 2>&1 < /dev/null &
  sleep 6

  if ! pgrep -f "easytier-core.*instance-name b" > /dev/null 2>&1; then
    echo "FAIL - B 进程已退出"
    cat /tmp/eb.log
    exit 1
  fi

  if ip addr show tun0 2>/dev/null | grep -q "inet 10.200.2.1"; then
    echo "READY - B OK"
  else
    echo "FAIL - B TUN not ready"
    cat /tmp/eb.log
    exit 1
  fi

  ip route add 10.200.1.0/24 dev tun0 2>/dev/null

  echo "=== Ping 测试 ==="
  if ping -c 2 -W 2 10.200.1.1; then
    echo "=== iperf3 测速 TCP 10s ==="
    iperf3 -c 10.200.1.1 -p 19999 -t 10 -J 2>/dev/null | grep -E "bits_per_second|sum_sent"
  else
    echo "Ping不通, 跳过iperf3"
    echo "--- B日志最后5行 ---"
    tail -5 /tmp/eb.log
    echo "--- A日志最后5行 ---"
    ssh root@192.168.1.37 'tail -5 /tmp/ea.log' 2>/dev/null || echo "(无法ssh到37取日志)"
  fi

else
  echo "用法: bash easy_bench.sh a|b [stealth] [secure]"
fi
