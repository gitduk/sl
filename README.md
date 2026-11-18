# sl

SSH port forwarding, with a panel.

`sl` keeps one supervised `ssh` process per forward, restarts it when the link
drops, and shows every row — state, uptime, retries, the ssh error text —
in a terminal panel. The same engine runs headless, so the CLI keeps working
without a terminal.

## Install

```bash
cargo install --path .
```

## Usage

```
sl                            the panel
sl wk:8000                    localhost:8000 → wk:8000
sl wk:4000,4001,4002          several ports on one host
sl 8001:wk:8000               localhost:8001 → wk:8000
sl -r 5056:wk:8080            wk:5056 → localhost:8080 (remote direction)
sl -d wk:8000                 background, returns at once
sl -k / sl -k wk              stop every session / those touching wk
```

Patterns are either `<host>:<port>[,<port>...]` (same port on both sides) or
`<entry>:<host>:<target>`. Local forwards listen on `127.0.0.1` only; a remote
forward listens wherever the far end's `sshd` allows — with `GatewayPorts no`
(the default) that is the far loopback, with `GatewayPorts yes` every interface.

## Panel

```
┌ sl — 3 rows, 2 up ───────────────────────────────────────────────────────┐
│      Host  Listen   Target     State      Up     Retry  Src              │
│[L]   wk    :8000    wk:8000    up         12m    -      panel            │
│[x]   ds    :8008    ds:8000    port in use -      3      panel            │
│[R]   wk    wk:5056  :8080     up         2m     -      daemon           │
├ log · :8008 ⇒ ds:8000 ───────────────────────────────────────────────────┤
│bind [127.0.0.1]:8008: Address already in use                            │
└──────────────────────────────────────────────────────────────────────────┘
 j/k select  n new  e edit  d delete  enter restart  space run/stop  t test
```

| key | action |
| --- | --- |
| `j` `k` / arrows | select |
| `g` `G` | first / last row |
| `n` | new forward |
| `e` | edit the selected forward |
| `d` | delete (asks first) |
| `enter` | restart the selected forward |
| `space` | start or stop the selected row |
| `a` `s` | start / stop every row |
| `t` | probe the local listen port |
| `/` | filter by host or port |
| `?` | keys |
| `q` | quit; stops the rows the panel started |

Rows started with `sl -d` show up as `src: daemon`; they survive the panel and
are stopped with `sl -k`.

The panel runs ssh with `BatchMode=yes`, so it never blocks on a password
prompt — set up a key or an agent first. The headless CLI keeps interactive
authentication.

## Config

`~/.config/sl/config.toml`, written by the panel, editable by hand:

```toml
[options]
retry_secs = 5
keepalive_secs = 15
keepalive_count_max = 3
up_grace_ms = 500

[[forward]]
host = "wk"
direction = "local"
entry = 8000
target = 8000
enabled = true
```

Rows that would fight over one listen port are refused when the file is read.

## Runtime state

`${XDG_RUNTIME_DIR:-/tmp}/sl/` (mode 0700): one `*.json` record and one `*.log`
of ssh stderr per row. `sl -k` only signals a pid whose command line still
carries the forward it was started with, so a recycled pid is never hit. A
panel that was killed leaves orphaned `ssh` children behind; the next panel
reclaims them on start.
