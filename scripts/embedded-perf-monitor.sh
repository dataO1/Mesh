#!/bin/bash
# Mesh Embedded Performance Monitor
# Designed for Orange Pi 5 (RK3588S) running mesh-player under NixOS
# No external dependencies beyond coreutils/procps/awk (no bc needed)
#
# Usage:
#   scp embedded-perf-monitor.sh mesh@<IP>:/tmp/
#   ssh mesh@<IP> 'nohup bash /tmp/embedded-perf-monitor.sh > /tmp/mesh-perf/monitor.log 2>&1 &'
#
# Outputs:
#   /tmp/mesh-perf/rt_settings.txt      - RT environment verification (rlimits, SCHED_FIFO, mlockall, etc)
#   /tmp/mesh-perf/pinning_snapshot.csv  - Thread pinning + scheduling policy at start
#   /tmp/mesh-perf/system.csv            - Overall CPU, RAM, GPU, temps every 5s
#   /tmp/mesh-perf/per_core.csv          - Per-core CPU usage (8 cores, big.LITTLE)
#   /tmp/mesh-perf/threads.csv           - Per-thread CPU usage for mesh-player
#   /tmp/mesh-perf/peak_cache.csv         - Peak cache hit/miss stats (from PEAK_CACHE log)
#   /tmp/mesh-perf/mesh-player.log       - Application log (journalctl)
#   /tmp/mesh-perf/monitor.log           - Human-readable status lines
#
# Stop: kill $(pgrep -f embedded-perf-monitor)

INTERVAL=${1:-5}
OUTDIR=/tmp/mesh-perf
mkdir -p "$OUTDIR"

# awk-based division helper (no bc dependency)
div10() { awk "BEGIN {printf \"%.1f\", $1 / 10}"; }
div1000() { awk "BEGIN {printf \"%.1f\", $1 / 1000}"; }

MESH_PID=$(pgrep -x mesh-player)
if [ -z "$MESH_PID" ]; then
    echo "ERROR: mesh-player not found"
    exit 1
fi

echo "Monitoring mesh-player PID=$MESH_PID (interval=${INTERVAL}s)"
echo "Output: $OUTDIR/"

# ---- Capture mesh-player logs in background ----
# Follow user journal (mesh-player logs via systemd-cat -t mesh-player)
journalctl --user --no-pager -f -o short-precise > "$OUTDIR/mesh-player.log" 2>/dev/null &
JOURNAL_PID=$!
# Fallback: also capture stderr from the process if accessible
if [ -f /proc/$MESH_PID/fd/2 ]; then
    tail -f /proc/$MESH_PID/fd/2 >> "$OUTDIR/mesh-player-stderr.log" 2>/dev/null &
    STDERR_PID=$!
fi

# Also grab dmesg for kernel-level events (OOM, thermal throttle, etc)
dmesg -w --time-format iso > "$OUTDIR/dmesg.log" 2>/dev/null &
DMESG_PID=$!

cleanup() {
    kill $JOURNAL_PID $DMESG_PID ${STDERR_PID:-} 2>/dev/null
    echo "Monitoring stopped. Files in $OUTDIR/"
}
trap cleanup EXIT

# ---- Snapshot RT settings and environment at start ----
{
    echo "=== RT Environment Snapshot ==="
    echo "Date: $(date)"
    echo "mesh-player PID: $MESH_PID"
    echo ""

    # Process rlimits (the critical ones)
    echo "--- Process Resource Limits (from /proc/$MESH_PID/limits) ---"
    if [ -f /proc/$MESH_PID/limits ]; then
        grep -E "rtprio|locked|nice|scheduling" /proc/$MESH_PID/limits 2>/dev/null
    fi
    echo ""

    # Scheduling policy per thread (SCHED_FIFO vs SCHED_OTHER)
    echo "--- Thread Scheduling Policies ---"
    for tid in $(ls /proc/$MESH_PID/task/ 2>/dev/null); do
        name=$(cat /proc/$MESH_PID/task/$tid/comm 2>/dev/null)
        # Field 41 in /proc/stat = policy (0=OTHER, 1=FIFO, 2=RR)
        # Field 18 = priority (RT priority for FIFO/RR)
        policy_num=$(awk '{print $41}' /proc/$MESH_PID/task/$tid/stat 2>/dev/null)
        rt_prio=$(awk '{print $18}' /proc/$MESH_PID/task/$tid/stat 2>/dev/null)
        case "$policy_num" in
            0) policy="SCHED_OTHER" ;;
            1) policy="SCHED_FIFO" ;;
            2) policy="SCHED_RR" ;;
            *) policy="UNKNOWN($policy_num)" ;;
        esac
        aff=$(taskset -pc $tid 2>/dev/null | cut -d: -f2 | tr -d ' ')
        echo "  TID $tid [$name]: $policy prio=$rt_prio affinity=$aff"
    done
    echo ""

    # PipeWire RT status
    echo "--- PipeWire Process ---"
    PW_PID=$(pgrep -x pipewire 2>/dev/null | head -1)
    if [ -n "$PW_PID" ]; then
        echo "PipeWire PID: $PW_PID"
        grep -E "rtprio|locked" /proc/$PW_PID/limits 2>/dev/null
        for tid in $(ls /proc/$PW_PID/task/ 2>/dev/null | head -10); do
            name=$(cat /proc/$PW_PID/task/$tid/comm 2>/dev/null)
            policy_num=$(awk '{print $41}' /proc/$PW_PID/task/$tid/stat 2>/dev/null)
            rt_prio=$(awk '{print $18}' /proc/$PW_PID/task/$tid/stat 2>/dev/null)
            case "$policy_num" in
                0) policy="SCHED_OTHER" ;;
                1) policy="SCHED_FIFO" ;;
                2) policy="SCHED_RR" ;;
                *) policy="UNKNOWN($policy_num)" ;;
            esac
            echo "  TID $tid [$name]: $policy prio=$rt_prio"
        done
    else
        echo "PipeWire not found"
    fi
    echo ""

    # /dev/cpu_dma_latency access
    echo "--- /dev/cpu_dma_latency ---"
    if [ -c /dev/cpu_dma_latency ]; then
        ls -la /dev/cpu_dma_latency
        # Check if mesh-player has it open
        ls -la /proc/$MESH_PID/fd/ 2>/dev/null | grep cpu_dma_latency && echo "  mesh-player has fd open" || echo "  mesh-player does NOT have fd open"
    else
        echo "  Device not found"
    fi
    echo ""

    # mlockall status (VmLck in /proc/status)
    echo "--- Memory Lock Status ---"
    grep -E "VmLck|VmRSS|VmSize" /proc/$MESH_PID/status 2>/dev/null
    echo ""

    # Kernel RT parameters
    echo "--- Kernel RT Sysctls ---"
    for param in kernel.sched_rt_runtime_us kernel.sched_rt_period_us \
                 kernel.sched_latency_ns kernel.sched_min_granularity_ns \
                 kernel.sched_wakeup_granularity_ns vm.swappiness; do
        val=$(cat /proc/sys/$(echo $param | tr '.' '/') 2>/dev/null)
        echo "  $param = $val"
    done
    echo ""

    # CPU governor and frequency
    echo "--- CPU Governors & Frequencies ---"
    for i in 0 1 2 3 4 5 6 7; do
        freq=$(cat /sys/devices/system/cpu/cpu$i/cpufreq/scaling_cur_freq 2>/dev/null)
        gov=$(cat /sys/devices/system/cpu/cpu$i/cpufreq/scaling_governor 2>/dev/null)
        echo "  CPU$i: ${freq}kHz [$gov]"
    done
    echo ""

    # CPU idle state status (disabled on cores 0-3?)
    echo "--- CPU Idle States (A55 cores 0-3) ---"
    for cpu in 0 1 2 3; do
        for state in /sys/devices/system/cpu/cpu$cpu/cpuidle/state*/; do
            sname=$(cat ${state}name 2>/dev/null)
            disabled=$(cat ${state}disable 2>/dev/null)
            echo "  CPU$cpu/$sname: disabled=$disabled"
        done
    done
    echo ""

    # cage-tty1 service limits
    echo "--- cage-tty1 Service Limits ---"
    systemctl show cage-tty1 2>/dev/null | grep -E "LimitRTPRIO|LimitMEMLOCK|LimitNICE|IOScheduling|OOMScoreAdjust|CPUAffinity" || echo "  (could not query systemd)"
    echo ""

    # Quick pass/fail summary
    echo "=== RT Readiness Check ==="
    RTPRIO_SOFT=$(grep "Max realtime priority" /proc/$MESH_PID/limits 2>/dev/null | awk '{print $5}')
    MEMLOCK_SOFT=$(grep "Max locked memory" /proc/$MESH_PID/limits 2>/dev/null | awk '{print $4}')
    VMLCK=$(grep "VmLck" /proc/$MESH_PID/status 2>/dev/null | awk '{print $2}')

    # Check SCHED_FIFO on any rayon-audio thread
    FIFO_OK="FAIL"
    for tid in $(ls /proc/$MESH_PID/task/ 2>/dev/null); do
        name=$(cat /proc/$MESH_PID/task/$tid/comm 2>/dev/null)
        if echo "$name" | grep -q "rayon-audio"; then
            pol=$(awk '{print $41}' /proc/$MESH_PID/task/$tid/stat 2>/dev/null)
            [ "$pol" = "1" ] && FIFO_OK="OK"
            break
        fi
    done

    # Check cpu_dma_latency fd
    DMA_OK="FAIL"
    ls -la /proc/$MESH_PID/fd/ 2>/dev/null | grep -q cpu_dma_latency && DMA_OK="OK"

    [ "${RTPRIO_SOFT:-0}" -ge 70 ] 2>/dev/null && RTPRIO_STATUS="OK (${RTPRIO_SOFT})" || RTPRIO_STATUS="FAIL (${RTPRIO_SOFT:-?})"
    [ "$MEMLOCK_SOFT" = "unlimited" ] && MEMLOCK_STATUS="OK" || MEMLOCK_STATUS="FAIL (${MEMLOCK_SOFT:-?})"
    [ "${VMLCK:-0}" -gt 0 ] 2>/dev/null && MLOCK_STATUS="OK (${VMLCK} kB locked)" || MLOCK_STATUS="FAIL (0 kB locked)"

    echo "  RLIMIT_RTPRIO >= 70:      $RTPRIO_STATUS"
    echo "  RLIMIT_MEMLOCK unlimited:  $MEMLOCK_STATUS"
    echo "  mlockall active:           $MLOCK_STATUS"
    echo "  SCHED_FIFO on audio:       $FIFO_OK"
    echo "  /dev/cpu_dma_latency fd:   $DMA_OK"
    echo "================================"
} > "$OUTDIR/rt_settings.txt" 2>&1

echo "RT settings captured to $OUTDIR/rt_settings.txt"
# Print the summary to monitor.log too
grep -A20 "RT Readiness Check" "$OUTDIR/rt_settings.txt"

# ---- Snapshot CPU pinning at start ----
echo "timestamp,tid,name,affinity,sched_policy,rt_priority" > "$OUTDIR/pinning_snapshot.csv"
for tid in $(ls /proc/$MESH_PID/task/ 2>/dev/null); do
    name=$(cat /proc/$MESH_PID/task/$tid/comm 2>/dev/null)
    aff=$(taskset -pc $tid 2>/dev/null | cut -d: -f2 | tr -d ' ')
    policy_num=$(awk '{print $41}' /proc/$MESH_PID/task/$tid/stat 2>/dev/null)
    rt_prio=$(awk '{print $18}' /proc/$MESH_PID/task/$tid/stat 2>/dev/null)
    case "$policy_num" in
        0) policy="OTHER" ;;
        1) policy="FIFO" ;;
        2) policy="RR" ;;
        *) policy="?$policy_num" ;;
    esac
    echo "$(date +"%Y-%m-%d %H:%M:%S"),$tid,$name,$aff,$policy,$rt_prio" >> "$OUTDIR/pinning_snapshot.csv"
done

# ---- Headers ----
echo "timestamp,uptime_sec,cpu_user,cpu_sys,cpu_idle,cpu_iowait,mem_total_mb,mem_used_mb,mem_available_mb,mesh_cpu_pct,mesh_mem_pct,mesh_rss_mb,mesh_vss_mb,mesh_threads,gpu_load_pct,gpu_freq_mhz,temp_soc,temp_bigcore0,temp_bigcore1,temp_little,temp_gpu" > "$OUTDIR/system.csv"
echo "timestamp,cpu0,cpu1,cpu2,cpu3,cpu4,cpu5,cpu6,cpu7" > "$OUTDIR/per_core.csv"
echo "timestamp,tid,name,affinity,cpu_pct" > "$OUTDIR/threads.csv"
echo "timestamp,cache_stats" > "$OUTDIR/peak_cache.csv"

# ---- Initialize delta tracking ----

declare -A PREV_THREAD_UTIME
declare -A PREV_THREAD_STIME
PREV_TOTAL_JIFFIES=0

get_total_jiffies() {
    awk '/^cpu / {print $2+$3+$4+$5+$6+$7+$8+$9+$10+$11}' /proc/stat
}

PREV_TOTAL_JIFFIES=$(get_total_jiffies)
for tid in $(ls /proc/$MESH_PID/task/ 2>/dev/null); do
    stat=$(cat /proc/$MESH_PID/task/$tid/stat 2>/dev/null)
    if [ -n "$stat" ]; then
        utime=$(echo "$stat" | awk '{print $14}')
        stime=$(echo "$stat" | awk '{print $15}')
        PREV_THREAD_UTIME[$tid]=$utime
        PREV_THREAD_STIME[$tid]=$stime
    fi
done

declare -a PREV_CORE_TOTAL
declare -a PREV_CORE_IDLE
for i in 0 1 2 3 4 5 6 7; do
    vals=$(awk "/^cpu$i / {print \$2+\$3+\$4+\$5+\$6+\$7+\$8+\$9, \$5}" /proc/stat)
    PREV_CORE_TOTAL[$i]=$(echo $vals | cut -d' ' -f1)
    PREV_CORE_IDLE[$i]=$(echo $vals | cut -d' ' -f2)
done

read PREV_USER PREV_NICE PREV_SYS PREV_IDLE PREV_IOWAIT PREV_REST < <(awk '/^cpu / {print $2, $3, $4, $5, $6, $7+$8+$9+$10+$11}' /proc/stat)

SAMPLE=0
sleep "$INTERVAL"

while true; do
    SAMPLE=$((SAMPLE + 1))
    TS=$(date +"%Y-%m-%d %H:%M:%S")
    UPTIME=$(awk '{print int($1)}' /proc/uptime)

    # ---- Overall CPU (permille precision, output as %) ----
    read CUR_USER CUR_NICE CUR_SYS CUR_IDLE CUR_IOWAIT CUR_REST < <(awk '/^cpu / {print $2, $3, $4, $5, $6, $7+$8+$9+$10+$11}' /proc/stat)
    DTOTAL=$(( (CUR_USER-PREV_USER) + (CUR_NICE-PREV_NICE) + (CUR_SYS-PREV_SYS) + (CUR_IDLE-PREV_IDLE) + (CUR_IOWAIT-PREV_IOWAIT) + (CUR_REST-PREV_REST) ))
    if [ $DTOTAL -gt 0 ]; then
        CPU_USER=$(( ((CUR_USER-PREV_USER) + (CUR_NICE-PREV_NICE)) * 1000 / DTOTAL ))
        CPU_SYS=$(( (CUR_SYS-PREV_SYS) * 1000 / DTOTAL ))
        CPU_IDLE=$(( (CUR_IDLE-PREV_IDLE) * 1000 / DTOTAL ))
        CPU_IOWAIT=$(( (CUR_IOWAIT-PREV_IOWAIT) * 1000 / DTOTAL ))
    else
        CPU_USER=0; CPU_SYS=0; CPU_IDLE=1000; CPU_IOWAIT=0
    fi
    PREV_USER=$CUR_USER; PREV_NICE=$CUR_NICE; PREV_SYS=$CUR_SYS
    PREV_IDLE=$CUR_IDLE; PREV_IOWAIT=$CUR_IOWAIT; PREV_REST=$CUR_REST

    # ---- Per-Core CPU ----
    CORE_USAGE=""
    for i in 0 1 2 3 4 5 6 7; do
        vals=$(awk "/^cpu$i / {print \$2+\$3+\$4+\$5+\$6+\$7+\$8+\$9, \$5}" /proc/stat)
        ct=$(echo $vals | cut -d' ' -f1)
        ci=$(echo $vals | cut -d' ' -f2)
        dt=$((ct - PREV_CORE_TOTAL[i]))
        di=$((ci - PREV_CORE_IDLE[i]))
        if [ $dt -gt 0 ]; then
            usage=$(( (dt - di) * 1000 / dt ))
        else
            usage=0
        fi
        PREV_CORE_TOTAL[$i]=$ct
        PREV_CORE_IDLE[$i]=$ci
        [ -n "$CORE_USAGE" ] && CORE_USAGE="$CORE_USAGE,"
        CORE_USAGE="$CORE_USAGE$(div10 $usage)"
    done
    echo "$TS,$CORE_USAGE" >> "$OUTDIR/per_core.csv"

    # ---- Memory ----
    read MEM_TOTAL MEM_USED MEM_AVAIL < <(free -m | awk '/^Mem:/ {print $2, $3, $7}')

    # ---- mesh-player process stats ----
    if [ -d /proc/$MESH_PID ]; then
        MESH_STAT=$(ps -p $MESH_PID -o %cpu,%mem,rss,vsz,nlwp --no-headers 2>/dev/null)
        MESH_CPU=$(echo $MESH_STAT | awk '{print $1}')
        MESH_MEM=$(echo $MESH_STAT | awk '{print $2}')
        MESH_RSS=$(echo $MESH_STAT | awk '{printf "%.0f", $3/1024}')
        MESH_VSS=$(echo $MESH_STAT | awk '{printf "%.0f", $4/1024}')
        MESH_THREADS=$(echo $MESH_STAT | awk '{print $5}')
    else
        echo "mesh-player exited, stopping monitor"
        break
    fi

    # ---- GPU (Mali via devfreq) ----
    GPU_LOAD_RAW=$(cat /sys/class/devfreq/fb000000.gpu/load 2>/dev/null)
    GPU_LOAD=$(echo $GPU_LOAD_RAW | cut -d@ -f1)
    GPU_FREQ=$(cat /sys/class/devfreq/fb000000.gpu/cur_freq 2>/dev/null)
    GPU_FREQ_MHZ=$((GPU_FREQ / 1000000))

    # ---- Temperatures (millidegrees C) ----
    TEMP_SOC=$(cat /sys/class/thermal/thermal_zone0/temp 2>/dev/null)
    TEMP_BIG0=$(cat /sys/class/thermal/thermal_zone1/temp 2>/dev/null)
    TEMP_BIG1=$(cat /sys/class/thermal/thermal_zone2/temp 2>/dev/null)
    TEMP_LITTLE=$(cat /sys/class/thermal/thermal_zone3/temp 2>/dev/null)
    TEMP_GPU=$(cat /sys/class/thermal/thermal_zone5/temp 2>/dev/null)

    # Write main CSV row
    CPU_USER_F=$(div10 $CPU_USER)
    CPU_SYS_F=$(div10 $CPU_SYS)
    CPU_IDLE_F=$(div10 $CPU_IDLE)
    CPU_IOWAIT_F=$(div10 $CPU_IOWAIT)
    echo "$TS,$UPTIME,$CPU_USER_F,$CPU_SYS_F,$CPU_IDLE_F,$CPU_IOWAIT_F,$MEM_TOTAL,$MEM_USED,$MEM_AVAIL,$MESH_CPU,$MESH_MEM,$MESH_RSS,$MESH_VSS,$MESH_THREADS,$GPU_LOAD,$GPU_FREQ_MHZ,$TEMP_SOC,$TEMP_BIG0,$TEMP_BIG1,$TEMP_LITTLE,$TEMP_GPU" >> "$OUTDIR/system.csv"

    # ---- Per-Thread CPU usage ----
    CUR_TOTAL_JIFFIES=$(get_total_jiffies)
    DELTA_TOTAL=$((CUR_TOTAL_JIFFIES - PREV_TOTAL_JIFFIES))
    NCPU=8

    for tid in $(ls /proc/$MESH_PID/task/ 2>/dev/null); do
        stat=$(cat /proc/$MESH_PID/task/$tid/stat 2>/dev/null)
        if [ -n "$stat" ]; then
            name=$(cat /proc/$MESH_PID/task/$tid/comm 2>/dev/null)
            aff=$(taskset -pc $tid 2>/dev/null | cut -d: -f2 | tr -d ' ')
            utime=$(echo "$stat" | awk '{print $14}')
            stime=$(echo "$stat" | awk '{print $15}')
            prev_u=${PREV_THREAD_UTIME[$tid]:-$utime}
            prev_s=${PREV_THREAD_STIME[$tid]:-$stime}
            dthread=$(( (utime - prev_u) + (stime - prev_s) ))
            if [ $DELTA_TOTAL -gt 0 ]; then
                thread_pct=$(awk "BEGIN {printf \"%.1f\", $dthread * $NCPU * 100 / $DELTA_TOTAL}")
            else
                thread_pct="0.0"
            fi
            PREV_THREAD_UTIME[$tid]=$utime
            PREV_THREAD_STIME[$tid]=$stime
            echo "$TS,$tid,$name,$aff,$thread_pct" >> "$OUTDIR/threads.csv"
        fi
    done
    PREV_TOTAL_JIFFIES=$CUR_TOTAL_JIFFIES

    # ---- Peak Cache Stats (from mesh-player journal) ----
    # Extract the most recent PEAK_CACHE log line (emitted every ~5s by mesh-player)
    CACHE_LINE=$(journalctl --user -t mesh-player --since "-${INTERVAL}s" --no-pager -o cat 2>/dev/null | grep "PEAK_CACHE" | tail -1)
    if [ -n "$CACHE_LINE" ]; then
        echo "$TS,$CACHE_LINE" >> "$OUTDIR/peak_cache.csv"
        CACHE_SHORT=$(echo "$CACHE_LINE" | sed 's/.*\[PEAK_CACHE\] //')
    else
        CACHE_SHORT=""
    fi

    # Human-readable status line
    CPU_TOTAL_F=$(awk "BEGIN {printf \"%.1f\", ($CPU_USER+$CPU_SYS) / 10}")
    TEMP_SOC_F=$(div1000 $TEMP_SOC)
    STATUS="[$TS] #$SAMPLE | CPU: ${CPU_TOTAL_F}% | RAM: $MEM_USED/${MEM_TOTAL}MB | mesh: $MESH_CPU% CPU, ${MESH_RSS}MB RSS | GPU: $GPU_LOAD% @${GPU_FREQ_MHZ}MHz | SoC: ${TEMP_SOC_F}C"
    [ -n "$CACHE_SHORT" ] && STATUS="$STATUS | $CACHE_SHORT"
    echo "$STATUS"

    sleep "$INTERVAL"
done
