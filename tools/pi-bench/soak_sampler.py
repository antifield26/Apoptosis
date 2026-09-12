"""P09 Pi soak sampler: RSS + CPU% for the mc-server service, every 10 s.

Writes CSV to stdout (redirect to ~/soak_metrics.csv): timestamp_s, rss_kb, cpu_percent.
CPU% is the utime+stime jiffy delta over the interval divided by the interval
and the core count is NOT divided out (100% = one full core), matching
systemd-cgtop conventions.
"""
import subprocess
import sys
import time

def main():
    duration_s = int(sys.argv[1]) if len(sys.argv) > 1 else 1900
    started = time.time()
    pid = subprocess.run(
        ["systemctl", "show", "-p", "MainPID", "--value", "mc-server"],
        capture_output=True, text=True).stdout.strip()
    if not pid.isdigit():
        raise SystemExit("mc-server MainPID not found: %r" % pid)
    print("ts_s,rss_kb,cpu_percent")
    prev_jiffies = None
    prev_t = None
    while time.time() - started < duration_s:
        try:
            rss = None
            with open("/proc/%s/status" % pid) as fh:
                for line in fh:
                    if line.startswith("VmRSS:"):
                        rss = int(line.split()[1])
                        break
            with open("/proc/%s/stat" % pid) as fh:
                fields = fh.read().rsplit(")", 1)[1].split()
            utime, stime = int(fields[11]), int(fields[12])
            jiffies = utime + stime
            clk = 100  # _SC_CLK_TCK on Linux
            cpu = None
            if prev_jiffies is not None:
                dt = time.time() - prev_t
                cpu = round((jiffies - prev_jiffies) / clk / dt * 100.0, 1)
            prev_jiffies, prev_t = jiffies, time.time()
            print("%d,%s,%s" % (int(time.time()), rss, cpu), flush=True)
        except FileNotFoundError:
            print("%d,PROCESS_GONE," % int(time.time()), flush=True)
            return
        time.sleep(10)

if __name__ == "__main__":
    main()
