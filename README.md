# pi-dash

A one-window terminal dashboard for the Raspberry Pi running
[ClassG](https://github.com/) — the three things a process monitor cannot tell
you, plus a system summary so you do not need one.

```
 pi-dash  classg-pi  127.0.0.1:8081                                                        17:38:30
┌ System ─────────────────────────────── up 3d4h5m ┐┌ Pi health ───────────────────────────────────┐
│         ⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀ ││  temp   58.4C ██████▎░░░░░  30-85            │
│         ⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⣴⣿⡇⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀ ││  power  0.8563V core   clock 1500/1800 MHz   │
│         ⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⣿⣿⡇⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀ ││  thrott OK  nothing right now                │
│         ⡀⡀⡀⢀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⣿⣿⡇⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀ ││  since  under-voltage, throttled  (0x50000)  │
│         ⢀⣤⣾⣿⣿⣦⣄⡀⡀⢀⣀⡀⡀⡀⡀⡀⡀⣿⣿⣧⣤⣤⣤⣶⣿⣷⣦⡀⡀⡀⡀⡀⡀⡀⡀⡀⡀⢀⣴⣿ ││  disk   21.0G/56.0G  38%                     │
│         ⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣦⣄⣀⣤⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣦⣤⣴⣶⣿⣿⣶⣶⣶⣿⣿⣿ ││  io     r 0 B/s   w 84.0 KB/s                │
│  cpu    ███▉░░░░░░░░░░░░  24%  4 cores           ││  api    no successful poll yet               │
│  c0  ██▉░░░░░░░░░░  22%  c1  █▉░░░░░░░░░░░  14%  │└──────────────────────────────────────────────┘
│  c2  █████▍░░░░░░░  41%  c3  █▏░░░░░░░░░░░   9%  │┌ Radios & network ────────────────────────────┐
│  mem    ██████▏░░░░░░░░░  38%  2.9G/7.6G         ││  wlan1   up   v1.2M   ^0B     monitor ch6    │
│  cache  ████▏░░░░░░░░░░░  26%  1.9G reclaimable  ││  eth0    up   v4.0K   ^2.0K                  │
│  swap   off   214 tasks, 2 running               ││                                              │
│                                                  ││  USB radios                                  │
│  PID     COMMAND                    MEM    CPU%  ││  0e8d:7961  MediaTek ALFA AWUS036AXML        │
│  1284    classg-api                120M    41.2  ││  0bda:2838  Realtek RTL2838 (RTL-SDR V4)     │
│  1290    classg_wifi                64M    18.7  │└──────────────────────────────────────────────┘
│  1301    classg-fusion               9M     4.1  │┌ ClassG  127.0.0.1:8081 ──────────────────────┐
│  902     dockerd                   206M     1.4  ││  ok   up 75h 25m   0.4.1                     │
│                                                  ││  sensors                                     │
│                                                  ││   wifi-1     wifi ok      1s 5m:12           │
│                                                  ││   sdr-1      sdr  ok      2s 5m:0            │
└──────────────────────────────────────────────────┘└──────────────────────────────────────────────┘
                                   q quit · r refresh now · ? help
```

It is a Rust rewrite of `classg/scripts/pi-dash.sh`, which orchestrated tmux
around a btop pane and three Bash readers. Everything is rendered by one
process now: **no tmux, no btop, no python3**.

## Why it reads `/proc` and `/sys` directly

Inherited from the Bash version, and still the right call. `iotop`, `nethogs`
and `bandwhich` all need root or `CAP_NET_ADMIN` to say anything useful, none
of them ship on a stock Pi OS, and the numbers they would add — per-process
disk and network — are not the numbers that break this box. The ones that do:

- an **under-volting supply**, which drops USB radios long before it shows up
  in any CPU graph,
- an **adapter that dropped off the USB bus**,
- a **wlan that fell out of monitor mode**,

are all readable unprivileged. So this runs as a normal user, and every reader
degrades to "unknown" rather than failing when the file or tool is not there —
which is also what makes the binary runnable on a dev machine that is not a Pi.

## Panes

| Pane | What |
|---|---|
| **system** | CPU history graph, aggregate and per-core meters, memory and reclaimable cache, swap, load, task counts, and the busiest processes, from `/proc/stat`, `/proc/meminfo`, `/proc/<pid>/stat` |
| **health** | temperature, core voltage, ARM clock, **decoded throttle bits**, disk, I/O |
| **radios** | per-interface throughput from `/proc/net/dev`, monitor-mode state, USB radio presence |
| **classg** | `GET /api/v1/health`, `/tracks`, `/detections` — degraded, never fatal |

### Why the system pane looks like btop

Because that is the pane btop used to occupy, and the layout is the one the
muscle memory is for: a scrolling history graph over a gradient meter, per-core
meters in a grid, then the memory split, then the busiest processes. Meters
fill at eighth-of-a-cell precision and colour green→amber→red *by position*,
so where a bar ends reads without stopping to parse the number next to it.

The history graph earns its rows on its own: a Pi pinned at 100% and a Pi that
spikes to 100% once a minute show the same instant number and are completely
different problems. Braille packs two samples per column and four levels per
row, so the graph holds roughly four minutes at the default cadence.

None of this is a btop reimplementation — no tree view, no process management,
no per-core history.

#### On the Pi's own HDMI console

The framebuffer console (`TERM=linux`) runs a font with no block, braille or
box-drawing glyphs; all three come out as replacement characters. Set

```toml
[dash]
glyphs = "ascii"
```

and the meters, the graph *and the pane frames* all drop to ASCII together.
The colour ramp also detects a 16-colour terminal from `TERM`/`COLORTERM` and
falls back on its own — no configuration needed for that half.

### The throttle bits

`vcgencmd get_throttled` carries each of its four conditions twice: bits 0-3
are *right now*, bits 16-19 are *has happened since boot*. Both are shown, on
separate rows, worded differently:

```
  thrott UNDER-VOLTAGE NOW, throttled
  since  under-voltage, throttled  (0x50005)
```

`0x50000` with a clean low nibble means it already happened and you missed it —
a different problem from `0x50005`, and it has to look different.

If `vcgencmd` is not present the pane says **unknown**, not OK. (The Bash
version defaulted the register to 0 and printed "clean since boot", which is a
confident lie on exactly the machines that cannot tell.)

## Running

```sh
cargo build --release
./target/release/pi-dash
```

Keys: `q`/`Esc`/`Ctrl-C` quit · `r` sample now · `?` help · `Ctrl-L` repaint ·
`Tab`/`1`-`4` switch pane (narrow terminals only).

To get it on `PATH` as `pidash`:

```sh
sudo ./install.sh              # or: ./install.sh ~/.local/bin
```

That writes a wrapper pointing at this checkout's release build, so rebuilding
here takes effect without reinstalling.

Below 100 columns the two-column layout leaves both halves unreadable, so the
dashboard shows one pane at a time instead — the same trade the Bash version
made by giving btop its own tmux window.

### Without a terminal

```sh
pi-dash --once          # one plain-text snapshot, no TTY needed
pi-dash --print-config  # resolved settings and where they came from
```

`--once` is how to check the readers against real hardware over SSH, from
cron, or from a CI step. It takes two samples ~0.7 s apart, because every rate
here is a difference between two readings.

## Configuration

Precedence: **env > file > defaults**.

| Variable | Default |
|---|---|
| `CLASSG_API` | `http://127.0.0.1:8081` |
| `CLASSG_DASH_INTERVAL` | `2` (seconds; fractional accepted) |

File-only settings worth knowing about: `theme` (accent colour), `processes`
(rows of process table), and `glyphs` (`unicode` or `ascii`, above).

See [`pi-dash.toml`](pi-dash.toml) for the file form. It is searched next to
the binary, in the working directory, then `~/.config/pi-dash/pi-dash.toml`;
missing is fine, and `--config` names one explicitly.

## Target and building

Built for **aarch64 Linux (Raspberry Pi OS Bookworm, kernel 6.12)**, where it
compiles natively — no cross toolchain needed:

```sh
sudo apt install -y build-essential   # rustup handles the rest
cargo build --release
```

There is no TLS in the dependency tree. The API is on loopback by default, and
linking rustls to reach `127.0.0.1` is not a trade worth making on a Pi, so an
`https://` base URL is reported as unsupported rather than failing obscurely.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Every parser has fixture-driven tests — throttle decoding, `/proc/net/dev`,
`/proc/stat` deltas, `/proc/<pid>/stat` with a comm full of parentheses,
`/proc/diskstats` partition de-duplication, USB ID matching — and the panes
themselves are rendered through ratatui's `TestBackend`, so a layout that
truncates a throttle verdict fails the build rather than the operator.

## Relationship to the classg repo

Standalone. It is **not** wired up as a git submodule of `classg`: that needs a
remote URL that does not exist yet.
