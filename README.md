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
```

Forwarding outlives the panel: `q` leaves the tunnels running and the next `sl`
adopts them, so you do not have to keep the panel open. A headless run
(`sl wk:8000`) is the exception — it stops on Ctrl-C.

Patterns are either `<host>:<port>[,<port>...]` (same port on both sides) or
`<entry>:<host>:<target>`. Local forwards listen on `127.0.0.1` only; a remote
forward listens wherever the far end's `sshd` allows — with `GatewayPorts no`
(the default) that is the far loopback, with `GatewayPorts yes` every interface.

## Panel

```
┌ sl — 3 rows, 2 up ───────────────────────────────────────────────────────┐
│      Host  Listen   Target     State      Up     Retry                   │
│[L]   wk    :8000    wk:8000    up         12m    -                       │
│[x]   ds    :8008    ds:8000    port in use -      3                      │
│[R]   wk    wk:5056  :8080     up         2m     -                       │
├ log · :8008 ⇒ ds:8000 ───────────────────────────────────────────────────┤
│bind [127.0.0.1]:8008: Address already in use                            │
└──────────────────────────────────────────────────────────────────────────┘
 j/k select  a add  e edit  d delete  r restart  space run/stop  t test
```

| key | action |
| --- | --- |
| `j` `k` / arrows | select |
| `g` `G` | first / last row |
| `a` | add a forward |
| `e` | edit the selected forward |
| `d` | delete (asks first) |
| `r` | restart the selected forward |
| `R` | restart every row |
| `space` | start or stop the selected row |
| `s` `S` | start / stop every row |
| `t` | probe the local listen port |
| `/` | filter by host or port |
| `?` | keys |
| `q` | quit; running forwards keep running |

Inside the editor: `←→` move the cursor, `ctrl-w` deletes a word, `ctrl-a` /
`ctrl-e` jump to the ends, `delete` removes forward, `tab` switches direction,
`esc` cancels. Pasted text arrives as one string (bracketed paste), so a copied
comma list lands in a single go.

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
of ssh stderr per row. The record is what lets a tunnel outlive its panel: each
`ssh` runs in its own session, so closing the terminal does not hang it up, and
the next panel adopts a record whose pid still carries that forward instead of
starting a second connection. Stop an adopted forward from the panel (`space`
or `d`) or by killing its pid.
