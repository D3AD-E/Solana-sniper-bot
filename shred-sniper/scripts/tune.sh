#!/bin/bash
# Host tuning for a latency-sensitive sniper on bare metal.
#
#   sudo ./scripts/tune.sh            apply the runtime settings
#   sudo ./scripts/tune.sh --pin      also pin the running process's threads
#
# The boot-time half (isolcpus, nohz_full, rcu_nocbs) cannot be set from here; see INFRA.md.
set -euo pipefail

HOT_CORE=${HOT_CORE:-6}      # the deshred/detect thread
SEND_CORE=${SEND_CORE:-7}    # the provider sender threads
NIC=${NIC:-}                 # e.g. enp1s0f0; autodetected when empty

say() { printf '%-52s %s\n' "$1" "$2"; }

# --- CPU ------------------------------------------------------------------
if command -v cpupower >/dev/null; then
  cpupower frequency-set -g performance >/dev/null 2>&1 || true
  say "governor" "performance"
else
  for f in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
    [ -w "$f" ] && echo performance > "$f" || true
  done
  say "governor" "performance (sysfs)"
fi

# Deep C-states cost microseconds to wake from, which is the same order as the whole
# detect path. Keep the cores shallow.
if [ -w /dev/cpu_dma_latency ]; then
  # holding this fd open is what actually pins the latency; done by the systemd unit
  say "cpu_dma_latency" "writable (see INFRA.md for the latency hog unit)"
fi

# --- UDP receive: the shred firehose --------------------------------------
# A dropped shred is a missed FEC set is a missed launch. Give the socket real buffers.
sysctl -qw net.core.rmem_max=134217728
sysctl -qw net.core.rmem_default=134217728
sysctl -qw net.core.netdev_max_backlog=250000
sysctl -qw net.core.optmem_max=4194304
say "udp receive buffers" "128MB max, backlog 250k"

# --- TCP: the provider connections ----------------------------------------
# The big one. Connections sit idle between launches, and by default the kernel throws away
# the congestion window after one RTO of idleness, so the first send after a quiet spell is
# slow-started. The 50s keep-alive pings do not prevent this on their own.
sysctl -qw net.ipv4.tcp_slow_start_after_idle=0
sysctl -qw net.ipv4.tcp_congestion_control=bbr 2>/dev/null || \
  sysctl -qw net.ipv4.tcp_congestion_control=cubic
sysctl -qw net.ipv4.tcp_notsent_lowat=16384
sysctl -qw net.ipv4.tcp_fastopen=3
sysctl -qw net.ipv4.tcp_syn_retries=3
sysctl -qw net.core.wmem_max=16777216
say "tcp_slow_start_after_idle" "0 (keeps the window warm between launches)"
say "tcp congestion control" "$(sysctl -n net.ipv4.tcp_congestion_control)"

# --- memory ---------------------------------------------------------------
sysctl -qw vm.swappiness=0
echo madvise > /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || true
echo madvise > /sys/kernel/mm/transparent_hugepage/defrag 2>/dev/null || true
say "swappiness / THP" "0 / madvise"

# --- NIC ------------------------------------------------------------------
if [ -z "$NIC" ]; then
  NIC=$(ip -o route get 1.1.1.1 2>/dev/null | awk '{print $5; exit}' || true)
fi
if [ -n "$NIC" ] && command -v ethtool >/dev/null; then
  # coalescing adds latency on purpose; we want the interrupt now
  ethtool -C "$NIC" adaptive-rx off rx-usecs 0 rx-frames 1 >/dev/null 2>&1 || true
  ethtool -G "$NIC" rx 4096 >/dev/null 2>&1 || true
  say "nic $NIC" "coalescing off, rx ring 4096"

  # keep NIC interrupts off the isolated cores
  if systemctl is-active --quiet irqbalance; then
    systemctl stop irqbalance && systemctl disable irqbalance
    say "irqbalance" "stopped (it would migrate IRQs onto the hot cores)"
  fi
  for irq in $(grep -l "$NIC" /proc/irq/*/smp_affinity_list 2>/dev/null | cut -d/ -f4); do
    echo 0-3 > "/proc/irq/$irq/smp_affinity_list" 2>/dev/null || true
  done
fi

# --- thread pinning -------------------------------------------------------
if [ "${1:-}" = "--pin" ]; then
  pid=$(pgrep -f jito-shredstream-proxy | head -1 || true)
  if [ -z "$pid" ]; then
    echo "no jito-shredstream-proxy running, nothing to pin"
    exit 0
  fi
  for t in /proc/"$pid"/task/*; do
    tid=$(basename "$t")
    name=$(cat "$t/comm" 2>/dev/null || echo "")
    case "$name" in
      shred_reconstructor)
        taskset -pc "$HOT_CORE" "$tid" >/dev/null
        chrt -f -p 80 "$tid" >/dev/null
        say "pinned $name ($tid)" "core $HOT_CORE, SCHED_FIFO 80"
        ;;
      snipeTx_*)
        taskset -pc "$SEND_CORE" "$tid" >/dev/null
        chrt -f -p 70 "$tid" >/dev/null
        say "pinned $name ($tid)" "core $SEND_CORE, SCHED_FIFO 70"
        ;;
      ssListen*)
        taskset -pc 0-3 "$tid" >/dev/null
        say "pinned $name ($tid)" "cores 0-3"
        ;;
    esac
  done
fi

echo
echo "Boot-time settings are not applied here. Add to the kernel command line:"
echo "  isolcpus=${HOT_CORE},${SEND_CORE} nohz_full=${HOT_CORE},${SEND_CORE} rcu_nocbs=${HOT_CORE},${SEND_CORE} processor.max_cstate=1 intel_idle.max_cstate=0"
