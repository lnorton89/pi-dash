# pi-dash

[![CI](https://github.com/lnorton89/pi-dash/actions/workflows/ci.yml/badge.svg)](https://github.com/lnorton89/pi-dash/actions/workflows/ci.yml)

A one-window terminal dashboard for the Raspberry Pi running
[ClassG](https://github.com/) — the three things a process monitor cannot tell
you, plus a system summary so you do not need one.

```
 pi-dash  classg-pi  127.0.0.1:8081                                                                                    21:28:27
┌ System ────────────────────────────────────────────────────── up 3d4h5m ┐┌ Pi health ────────────────────────────────────────┐
│                                        CPU ███▍░░░░░░░░░░  24%          ││  temp   58.4C ██████▎░░░░░  30-85                 │
│                      ⣤⣶⣶               c0  ███▏░░░░░░░░░░  22%  ⣀⣀⣀⣀⣀⣀⣠⣤││  power  0.8563V core   clock 1500/1800 MHz        │
│                      ⣿⣿⣿               c1  ██░░░░░░░░░░░░  14%  ⣀⣀⣀⣀⣤⣤⣤⣀││  thrott OK  nothing right now                     │
│                      ⣿⣿⣿               c2  █████▊░░░░░░░░  41%  ⣀⣠⣤⣤⣄⣀⣀⣀││  since  under-voltage, throttled  (0x50000)       │
│  ⣿⣷⣦⡀         ⢀⣴⣿⣿⣷⣤⣀⣿⣿⣿⣀     ⢀⣀⣀⣀⣀⣠⣴  c3  █▎░░░░░░░░░░░░   9%  ⣤⣤⣀⣀⣀⣀⣀⣀││  disk   21.0G/56.0G  38%                          │
│  ⣿⣿⣿⣿⣦⣤⣴⣶⣿⣿⣶⣶⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣤⣀⣠⣴⣿⣿⣿⣿⣿⣿⣿  load 0.52 0.31 0.20              ││  io     r 0 B/s   w 84.0 KB/s                     │
│  mem    █████████▏░░░░░░░░░░░░░░  38%  2.9G/7.6G                        ││  api    no successful poll yet                    │
│  cache  ██████▏░░░░░░░░░░░░░░░░░  26%  1.9G reclaimable                 │└───────────────────────────────────────────────────┘
│  swap   off   214 tasks, 2 running                                      │┌ Radios & network ─────────────────────────────────┐
│                                                                         ││  wlan1   up   v1.2M   ^0B     monitor ch6         │
│  PID     COMMAND                                 MEM    CPU%            ││  eth0    up   v4.0K   ^2.0K                       │
│  1284    classg-api                             120M    41.2 ████▏░░░░░ ││                                                   │
│  1290    classg_wifi                             64M    18.7 █▉░░░░░░░░ ││  USB radios                                       │
│  1301    classg-fusion                            9M     4.1 ▍░░░░░░░░░ ││  0e8d:7961  MediaTek ALFA AWUS036AXML             │
│  902     dockerd                                206M     1.4 ▏░░░░░░░░░ ││  0bda:2838  Realtek RTL2838 (RTL-SDR V4)          │
│  61      kworker/1:2-events                        0     0.4 ▏░░░░░░░░░ │└───────────────────────────────────────────────────┘
│                                                                         │┌ ClassG  127.0.0.1:8081 ───────────────────────────┐
│                                                                         ││  ok   up 75h 25m   0.4.1+a1b2c3d                  │
│                                                                         ││  store     libsql       11.5G free of 28.9G       │
│                                                                         ││  recording on                                     │
│                                                                         ││                                                   │
│                                                                         ││  sensors                                          │
│                                                                         ││   SENSOR     KIND  STATE  BEAT   5MIN             │
│                                                                         ││   wifi-1     wifi  ok       1s     12             │
│                                                                         ││   sdr-1      sdr   DOWN      -      0             │
│                                                                         ││     rtl_sdr: device not found                     │
│                                                                         ││                                                   │
│                                                                         ││  fusion    connected   last message 2s            │
│                                                                         ││                                                   │
│                                                                         ││  tracks 1 live                                    │
│                                                                         ││   STATE     CONF IDENTITY    EVID   DET SEEN      │
│                                                                         ││   CONFIRMED 0.82 Mavic 3     Ax402  402   3s      │
│                                                                         ││     120m agl  14m/s  -58dBm  held 4m              │
│                                                                         ││                                                   │
│                                                                         ││  detections 1284 total                            │
│                                                                         ││   TIME     CLASS          dBm  TUNE  ID           │
│                                                                         ││   14:32:08 Remote ID      -52 ch149  Mavic 3      │
└─────────────────────────────────────────────────────────────────────────┘└───────────────────────────────────────────────────┘
                                q quit · r refresh now · s sort by mem · ? help
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
| **system** | CPU history graph beside per-core meters and sparklines, memory and reclaimable cache, swap, load, task counts, and the busiest processes with their command lines, from `/proc/stat`, `/proc/meminfo`, `/proc/<pid>/stat` |
| **health** | temperature, core voltage, ARM clock, **decoded throttle bits**, disk, I/O |
| **radios** | per-interface throughput from `/proc/net/dev`, monitor-mode state, USB radio presence |
| **classg** | whether the unit is recording, which sensors are alive, what is holding a radio, and what is in the sky — degraded, never fatal |

### What the ClassG pane is actually for

An empty track list is the most ambiguous thing this dashboard can draw. It
means a quiet sky, or a paused recording, or a dead fusion link, or a session
that expired, or a sweep that took the radio thirty seconds ago — and the
consequences of confusing those are not symmetric. A detector that has stopped
detecting silently manufactures false confidence, which is worse than one that
is visibly offline (ClassG ADR-0003).

So everything above the track list is there to remove one way of misreading it:

| Line | From | Removes the reading |
|---|---|---|
| `recording PAUSED, 1.2k discarded` | `/monitoring` | ingestion is gated; sensors and fusion look perfectly healthy while nothing is recorded |
| `store libsql  1.0G free of 28.9G` | `/system` | the filesystem detections land on is filling, and it is not the one the health pane measures |
| sensor `DOWN` with its reason | `/health` | a radio that never came up, versus one that stopped |
| `fusion down` | `/health` | every sensor heartbeating into a track pipeline that is dead |
| `capture running wlan1 ch6` | `/captures` | the Wi-Fi sensor is quiet because something took its monitor interface |
| `sweep running 2.4GHz` | `/spectrum/sweeps` | ADS-B is quiet because a sweep borrowed the SDR from dump1090 (ADR-0008) |
| `refused: log in to continue` | `/auth/me` | the lists are empty because the API declined, not because the sky is |

Two details in the track and detection lists are also deliberate rather than
decorative:

- **A class is named, not lettered.** `Remote ID` and `Control link`, not `A`
  and `E`. A letter is only a claim if you have memorised `data-model.md`.
- **A contact nothing identified is marked `~` and dimmed.** Classes C, D and
  H corroborate an identification but never make one — an OUI names whoever
  built the radio, not what is flying it. This mirrors `corroboratingOnlyClasses`
  in `services/fusion/track.go`. Without it a DJI-branded access point sits in
  the list looking exactly like a real Remote ID contact, which is a mistake
  that has actually been made.

### Why the system pane looks like btop

Because that is the pane btop used to occupy, and the layout is the one the
muscle memory is for. It is btop's CPU box: a scrolling history graph on the
left, the per-core column on the right, each core with its own meter and
sparkline, the load average under them. Then the memory split, then the
process table.

Meters fill at eighth-of-a-cell precision and colour *by position* along the
bar, so where one ends reads without stopping to parse the number beside it.
The ramp is green→amber→red for anything where more is worse, and blue→cyan
for reclaimable page cache — which is not a warning at any level, and looked
like one when it shared the load ramp.

The history graph earns its rows on its own: a Pi pinned at 100% and a Pi that
spikes to 100% once a minute show the same instant number and are completely
different problems. Braille packs two samples per column and four levels per
row. Everything above the trace is blank, with a floor on the bottom row —
a dim character at every level turns the pane into graph paper and loses the
one row that carries the data.

It has grown towards btop rather than away from it: memory, disks and network
sit in one band across the pane, and the process table carries threads, the
owning account, a filter and a `12/374` count. What it still does not do is
manage anything — no tree view, no renice, no kill. This is a dashboard for a
box you are diagnosing over SSH, and every key on it is safe to press.

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

Keys: `q`/`Esc`/`Ctrl-C` quit · `r` sample now · `s` sort the process table by
CPU or by memory · `f` filter it by name or command line · `?` help ·
`Ctrl-L` repaint · `Up`/`Down`/`PgUp`/`PgDn`/`Home` scroll the process table ·
`Tab`/`1`-`4` focus a pane. Each pane's number is shown in its own title and
the focused one keeps the accent colour, so the keys do not need looking up.

While a filter is open every key is a letter, so `q` types a q rather than
quitting — `Enter` keeps the filter, `Esc` clears it, and `Ctrl-C` still gets
you out. It matches the comm *and* the command line, because `--net-ri-port`
lives in exactly one of the two; while one is set, every process gets its
command line read rather than only the rows the sort brought to the top.

The sorted column is coloured in the heading rather than marked with an arrow:
the columns are already exactly as wide as the numbers under them, and every
arrow worth reading is outside ASCII, which is the one thing the framebuffer
console cannot draw.

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
pi-dash --check         # one verdict line, and an exit code to branch on
pi-dash --print-config  # resolved settings and where they came from
```

`--once` is how to check the readers against real hardware over SSH. It takes
two samples ~0.7 s apart, because every rate here is a difference between two
readings, and it always exits 0 — a snapshot's job is to render, and it
rendered. For cron and CI, use `--check` below.

The header states that same verdict, so a glance answers the question four
panes of facts otherwise leave you to assemble: `ok`, or `degraded - recording
is paused (known local flight)`, or `down - the API is not answering`. It goes
through the same `judge` as `--check`, so the screen on the wall and the exit
code in your crontab cannot drift apart.

`--check` is the monitoring half. One line, and an exit code:

| Code | Means |
|---|---|
| `0` | ok |
| `1` | degraded — working, but something here ends badly if left alone |
| `2` | down — the API is unreachable, or says so itself |

```
$ pi-dash --check
degraded: the API reports degraded; sensor sdr-1 is down (rtl_sdr: device not
found); recording is paused (known local flight), 1204 detections discarded
```

It judges what the *dashboard* can see, not only what `/health` says: a
perfectly healthy API on a Pi that is browning out, or 90% through its card, is
a detector with a date on it. Specifically it fails on a paused recording, a
non-optional sensor down, an API refusal, a live under-voltage or throttle, and
any filesystem at 90% — not just `/`, because on a unit recording to a stick
that is the disk least likely to be the one filling up.

Two things it deliberately does *not* fail on. Optional hardware that was never
fitted — a Wi-Fi-only build has no SDR and must not fail for ever, or the check
is one you turn off. And the sticky throttle register: a brownout at three in
the morning is real history and belongs on the pane, but a probe that keeps
failing because of it is a probe somebody silences.

The line is always printed, so running it by hand says something. From cron,
redirect stdout if you only want mail when something is wrong:

```sh
*/5 * * * * /usr/local/bin/pidash --check >/dev/null
```

`--print-config` prints each resolved value with the tier that set it:

```
config    /home/pi/.config/pi-dash/pi-dash.toml
api       http://pi.local:9000                      (environment)
interval  2.00s                                     (config file)
api poll  3.00s                                     (config file)
session   set                                       (environment)
```

The right-hand column is the point — a dashboard pointed at the wrong box is
almost always a `CLASSG_API` still exported in the shell it was launched from,
and no amount of reading the config file reveals that. `session` reports only
whether one is configured, never its value.

## Configuration

Precedence: **command line > env > file > defaults**.

| Variable | Flag | Default |
|---|---|---|
| `CLASSG_API` | `--api <URL>` | `http://127.0.0.1:8081` |
| `CLASSG_DASH_INTERVAL` | `-i, --interval <SECONDS>` | `2` (seconds; fractional accepted) |
| `CLASSG_SESSION` | — | unset |

`-c, --config <FILE>` names a file instead of searching. A file named that way
and not found is an error; one merely absent from the search path is not.

`--print-config` reports which tier each value came from, which is the fastest
way to find a `CLASSG_API` still exported in the shell this was launched from.

File-only settings worth knowing about: `theme` (accent colour), `processes`
(rows of process table), `api_interval_secs`, and `glyphs` (`unicode` or
`ascii`, above).

### If the API has authentication switched on

Only `/health` and `/auth/me` are public. Everything else — tracks, detections,
the recording switch, the build string — needs a viewer session.

**On the unit itself this is automatic and needs no configuration.** The API
writes a local-agent token into the state directory it already shares with the
host-side deploy and watchdog agents, mode `0640`, and pi-dash finds it and
sends it as `Authorization: Bearer`. Being able to read that file is the
credential: a process running as the account that owns the deployment is the
operator, and the kernel is what says so. It grants **viewer** and nothing more,
so this can read the sky and cannot restart a radio or stop a recording.

Look-up order, first hit wins:

| | Where |
|---|---|
| `CLASSG_LOCAL_TOKEN` | an explicit path to the file |
| `CLASSG_DEPLOY_STATE` | the agents' own variable, `<dir>/local-api-token` |
| beside the checkout | `<repo>/.agent-state/local-api-token`, found by walking up from this binary |
| `~/.local/state/classg/local-api-token` | the agents' default outside a checkout |

`pi-dash --print-config` says which credential is in play, never its value.

### Watching a different unit

A token on this disk describes *this* box, so it is no use pointed at another
one. `CLASSG_SESSION` still exists for that and takes precedence over anything
found locally:

```sh
CLASSG_SESSION=<value of the classg_session cookie> pi-dash
```

That is a token, not a password. pi-dash never logs in and holds nothing that
could mint a session; copy the cookie from a browser that is already signed in.
It can also go in the config file as `session`, but the environment is the
better place for a live credential.

A unit with authentication off needs none of this, and a pane with no
credential at all still draws — degraded, and saying so.

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
cargo clippy --all-targets
cargo test
```

No `-- -D warnings`: the `[lints]` table in `Cargo.toml` already denies them,
along with `unwrap`, `expect`, `panic`, and a wildcard match over somebody
else's enum. Passing the flag again on the command line would hide whether that
table is doing its job.

Every parser has fixture-driven tests — throttle decoding, `/proc/net/dev`,
`/proc/stat` deltas, `/proc/<pid>/stat` with a comm full of parentheses and
with a line truncated mid-read, `/proc/diskstats` partition de-duplication, USB
ID matching — and the panes themselves are rendered through ratatui's
`TestBackend`, so a layout that truncates a throttle verdict fails the build
rather than the operator.

CI runs all three on Linux, which matters more than it sounds: half of this
crate reads `/proc`, `/sys` and `vcgencmd`, and on a Windows or macOS
development box those tests exercise the fallbacks rather than the readers. It
also renders `--once` against a real kernel and asserts that `--check` reports
a missing API as `down` with exit 2, and it fails on a CRLF line ending or a
NUL byte in a tracked file — both of which have already cost this project real
time.

## Relationship to the classg repo

Standalone. It is **not** wired up as a git submodule of `classg`: that needs a
remote URL that does not exist yet.
