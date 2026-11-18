# sl —— SSH 端口转发面板 设计计划

栈：Rust + ratatui 0.30 + crossterm 0.29（与 `~/ccs` 同一套），serde + toml，clap 4，libc。
不引入 tokio：UI 单线程、每会话一线程、mpsc 单向事件，够用且好测。

## 1. 目标

1. `sl` 无参进面板：一张转发表 + 选中条日志 + 状态栏，增删改、单条重启、测连。
2. 带 pattern 时保持现有一次性行为（`sl wk:8000`、`sl -r 5056:wk:8080`、`sl -d`、`sl -k`）。
3. 修掉现有脚本的已知缺陷：`$TMPDIR` 非 `/tmp` 时 `-k` 失效；重连后把所有端口都显示成成功；按 host 混在一起的状态无法定位到具体端口。

非目标：面板内的密码／passphrase 输入；IPv6 字面量 host（沿用现状）；代理跳板配置管理。

## 2. 现有脚本（`~/.local/bin/sl`，293 行 zsh）的行为基线

- 一个 host 一个 `ssh -M -S sock -f -N` 主控连接，所有 `-L/-R` 挂在同一条上。
- 状态靠 `tmp_dir/<host>.err` 正则抠失败端口（`^bind .+:([0-9]+): ` 与 `forwarding failed for listen port ([0-9]+)`），实测在当前 OpenSSH 10.2p1 上两个正则都能命中。
- 重连：`-O check` 每 5s 一次，失败即重挂。
- 会话目录 `mktemp -d -t sl.XXXXXX`，即受 `$TMPDIR` 影响；而 `-k` 只扫 `/tmp/sl.*` —— 两者不一致，`TMPDIR` 改过就产生杀不掉的会话。

## 3. 关键设计决策

### 3.1 一行一个 ssh 进程，不用 ControlMaster 复用

实测（OpenSSH 10.2p1，本机 → `wk`）：

| 方式 | 成功 | 本地端口被占 |
| --- | --- | --- |
| `ssh -N -L 127.0.0.1:P:localhost:22 -o ExitOnForwardFailure=yes` | 常驻，无输出 | 退出码 255，stderr 有 `bind [127.0.0.1]:18099: Address already in use` |
| 主控 + `ssh -O forward -L ...` | 退出码 0 | 退出码 255，但 stderr 只有 `mux_client_forward: forwarding request failed: Port forwarding failed`（具体端口和原因在主控进程的 stderr 里） |

选“一行一进程”：

- 失败原因文本完整（原因只在主控的 stderr 里时，多 host 混一个日志流，行与原因对不上）。
- 行状态 = 子进程是否存活，不需要继续解析 stderr（现有脚本最大的脆点）。
- 单行重启 = 重开该进程，不动别人；主控方案下主控一死，整台 host 的行要全部重挂。
- 实测 `-O cancel` 对不存在的转发也返回 0，主控方案的幂等性还得自己补。

代价：一台 host N 行 = N 条 SSH 连接（sshd 默认 `MaxStartups 10:30:100`，≤10 行无感）。行数真涨到几十条再换主控方案，届时 `-O forward` / `-O cancel` 已经实测可用。

### 3.2 一个引擎，三个入口

```
sl            → 面板（TUI 前端）
sl <pattern>  → 前台无头监督（今天的行为：阻塞、自动重连、Ctrl-C 清理）
sl -d <pattern> → 同上，脱离终端后返回
sl -k [host]  → 杀运行时目录里登记的会话
```

四个入口共用同一套 `sessions` 引擎，只有“事件消费方”不同（表格渲染 / stderr 打行）。不写第二套 ssh 生命周期代码。

### 3.3 会话与配置分离

配置（用户意图，持久）只存“要转发什么”；会话（当前状态，易失）只在运行时目录里，永远不写回配置。重试天然幂等。

### 3.4 面板内的 ssh 一律 `-o BatchMode=yes`

面板占着 tty 且处于 raw 模式，子进程即使 stdin 指 `/dev/null`，ssh 也可能去读 `/dev/tty` 要密码，把界面搅乱。一次性 CLI 路径不加这个参数（保留交互密码／口令）。

### 3.5 运行时目录固定为 `${XDG_RUNTIME_DIR:-/tmp}/sl`

不再 `mktemp`。一个目录，CLI 与面板、本次与前一次都能互相看见；权限 0700，`~` 之外不放东西。这是 `-k` 失效那个 bug 的根因修复，不是加特判。

## 4. 数据模型

```rust
// src/forward.rs —— 领域类型，只有校验和转义，没有 I/O
pub struct Host(String);       // 非空，不含 ':' ',' 空白
pub struct Port(u16);          // 1..=65535

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction { Local, Remote }

/// 一行的身份：这四项相同就是同一个转发行。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ForwardSpec {
    pub host: Host,
    pub direction: Direction,
    pub entry: Port,
    pub target: Port,
}

impl ForwardSpec {
    /// `<host>:<port>[,<port>...]` 与 `<entry>:<host>:<target>` 两种写法，
    /// 方向由调用方给（CLI 的 `-r`、面板新增框的选项）。
    pub fn parse_all(args: &[String], direction: Direction) -> Result<Vec<ForwardSpec>, SpecError>;
}
```

- `Direction` 是行属性，不再是全局开关；CLI 的 `-r` 只是“把这些 pattern 的方向都设成 Remote”的语法糖。
- 校验放在解析边界（`parse_all`、`Config::load`），内部类型构造后不再重复检查。

```rust
// src/config.rs
pub struct Config { pub options: Options, pub forward: Vec<Entry> }

pub struct Entry { pub spec: ForwardSpec, pub enabled: bool }

pub struct Options {
    pub retry_secs: u64,             // 默认 5
    pub keepalive_secs: u64,         // 默认 15
    pub keepalive_count_max: u32,    // 默认 3
    pub up_grace_ms: u64,            // 默认 500，见 6 节
}
```

配置内的冲突在建表时就拒绝，并指名两行：同一 `ForwardSpec` 重复；Local 方向下同一 `entry` 绑同一个 `127.0.0.1`；Remote 方向下同一 `(host, entry)`。

```rust
// src/sessions.rs —— 引擎，唯一持有子进程的地方
pub enum Status {
    Starting,
    Up { since: Instant },
    Failed { reason: String, at: Instant },
    Waiting { attempt: u32, retry_at: Instant },
    Stopped,
}

pub enum SessionEvent { Status { spec: ForwardSpec, status: Status } }

pub enum Command { Stop(ForwardSpec), Restart(ForwardSpec), StopAll }

pub struct Sessions { cmd_tx: Sender<Command>, evt_rx: Receiver<SessionEvent> }
impl Sessions {
    pub fn start(specs: Vec<ForwardSpec>, options: Options, owner: Owner) -> Result<Self, Error>;
}
```

一行一个线程，独占该行的子进程与重试计时；对外只暴露两个通道和一张状态表的读副本。UI 从不阻塞在 ssh 上。

## 5. 模块划分

```
src/main.rs        入口 + clap 定义，四个入口的分发（薄）
src/lib.rs
src/forward.rs     ForwardSpec/Direction/Host/Port + 解析与校验 + 单测
src/config.rs      ~/.config/sl/config.toml 读写、冲突校验、原子落盘（tmp + rename）
src/runtime.rs     运行时目录、会话记录文件、`-k` 的扫描与清理
src/sessions.rs    引擎：spawn/wait/重试/事件 + 单测（假 ssh）
src/report.rs      无头前端：把 SessionEvent 打成 `[L]/[R]/[x]` 行
src/tui/mod.rs     终端进出、事件循环、定时器（日志尾随 500ms、倒计时 1s）
src/tui/state.rs   App：条目表、选中、Mode、message、过滤串
src/tui/theme.rs   照抄 ccs 的 One Dark 调色板
src/tui/table.rs   主表 + Normal 模式键位
src/tui/editor.rs  新增／编辑表单（复用 ForwardSpec::parse_all 的字段级校验）
src/tui/logs.rs    选中条的日志尾随
src/tui/dialogs.rs 帮助、删除确认、退出确认
```

`tui/*` 只读 `Sessions`/`Config` 的快照、只写 `Command`，不直接碰 `std::process`。

## 6. 会话状态机（每行一个线程）

```
Stopped ──start──▶ Starting ──500ms 内未退出──▶ Up
                      │                          │
                      │ 退出码非 0               │ 子进程退出
                      ▼                          ▼
                   Failed ──5s──▶ Waiting ──▶ Starting（attempt += 1）
```

- `up_grace = 500ms`：ssh 绑定失败是立刻退出的（实测 255 在毫秒级），活过宽限期即认定转发已挂上。宽限期可配置，避免慢网络下把 Up 判成 Failed。
- `Failed.reason` 取该行日志文件最后一条非空行（就是 ssh 的原因），面板和 CLI 都显示这一条。
- Stop：置停止标志 + 对记录的 pid 发 `SIGTERM`，线程观察到退出后进 `Stopped`，不再重试。
- 重试不累积状态：每次都是全新 `ssh` 进程，`ExitOnForwardFailure=yes` 保证端口没挂上就一定失败退出。

## 7. 运行时目录布局

```
${XDG_RUNTIME_DIR:-/tmp}/sl/          0700
  L-wk-8000-8000-3fa2c1.json         会话记录（原子写）
  L-wk-8000-8000-3fa2c1.log          ssh stderr 追加（单文件上限 64 KiB，超了截半）
```

```json
{ "spec": {...}, "owner": "panel", "supervisor_pid": 1234, "ssh_pid": 1235, "started_at": 1763… }
```

- 文件名 = 方向 + host + 端口 + 规范串的 8 位哈希（host 允许 `_`、`-`，纯拼接会撞名）。
- `-k`：遍历 `.json`，`/proc/<pid>/cmdline` 必须含 `ssh` 且端口串对得上才发 TERM，然后删记录；对 `owner=daemon` 先杀 supervisor 再杀 ssh。今天的脚本只校验 `comm == zsh`，这条更严。
- `owner=daemon` 的行在面板里只读显示（可 `d` 杀），面板不接管它们的重连线程。

## 8. 配置文件

```toml
# ~/.config/sl/config.toml
[options]
retry_secs = 5
keepalive_secs = 15
keepalive_count_max = 3
up_grace_ms = 500

[[forward]]
host = "wk"
direction = "local"
entry = 4000
target = 4000
enabled = true
```

- 缺文件 = 空表，面板显示“按 n 添加”。
- 未知字段忽略（`serde(default)`），未知 `direction` 直接报错（静默当成 local 会让 `-R` 变 `-L`，错得很安静）。
- `options` 全部有默认值，用户不写就不出现。

## 9. CLI 表面

```
sl                           面板
sl wk:8000                   前台无头，localhost:8000 → wk:8000
sl wk:4000,4001,4002         多端口
sl 8001:wk:8000              本地 8001 → wk:8000
sl -r 5056:wk:8080           wk:5056 ← 本地 8080（远程转发）
sl -d wk:8000                后台跑，立即返回
sl -k / sl -k wk             杀掉登记过的会话
sl --help, sl --version
```

- `-r` 只影响本次命令行里解析出的行；配置文件里的 `direction` 各自独立。
- `--` 之后一律当 pattern（保留）。
- 不再有 `-h` 打印用法：`--help` 由 clap 生成，`?` 是面板内的快捷键。

## 10. 面板布局与键位

```
┌ sl ──────────────────────────────────── 3 up · 1 failed · 1 waiting ┐
│ 标记  主机   方向  入口      目标      状态              时长  重连  │
│ [L]  wk    local   :8000  ⇒ wk:8000    up                12m    0    │
│ [x]  ds    local   :8008  ⇒ ds:8000    bind: in use       —      3    │
│ [R]  wk    remote  wk:5056 ⇒ :8080     up                 2m     0    │
├ 日志 ────────────────────────────────────────────────────────────────┤
│   bind [127.0.0.1]:8008: Address already in use                      │
├──────────────────────────────────────────────────────────────────────┤
│ j/k 选择  n 新增  e 编辑  d 删除  Enter 重启  a 全启  s 全停  t 测连 │
│ / 过滤  ? 帮助  q 退出                                               │
└──────────────────────────────────────────────────────────────────────┘
```

- 竖直比约 3:2（表 : 日志），日志区最少 6 行，终端 < 24 行时日志折叠成一行。
- `t` 测连：Local 行连 `127.0.0.1:entry`（成功即隧道通）；Remote 行不做猜测，报“以 ssh 状态为准”。
- `q` 时若有 `Up` 行弹确认：“退出会停止 N 个会话”。
- 颜色沿用 ccs：`SUCCESS` 绿=up，`ERROR` 红=failed，`WARNING` 黄=waiting，`MUTED`=stopped，选中用 `HIGHLIGHT_BG`。

## 11. 边界与校验

| 边界 | 规则 |
| --- | --- |
| pattern 解析 | 端口 1..=65535；host 非空且不含 `:` `,` 空白；`<port>:<host>:<port>` 才认三段，其余报错并回显原串 |
| 配置文件 | 整份校验通过才生效，任一错误即拒绝启动并指出文件与行号 |
| 会话记录 | JSON 解析失败即视为陈旧记录，删除，不中断 |
| ssh 参数 | 端口与 host 经类型转义后拼接，不经过 shell（`Command::arg`），不做字符串插值 |
| 远端端口冲突 | 收到 ssh 的失败原因原样入日志，不翻译不改写 |

## 12. 测试计划

不联网、不依赖真实 sshd；用 `PATH` 注入假 `ssh` 脚本（`tests/fake_ssh/`：一个立即退出并打 bind 错误的、一个常驻到被 TERM 的）。

1. `forward::parse_all`：两种写法、逗号多端口、`-r` 方向映射、非法（`wk:0`、`wk:x`、`a:b:c:8000`、空 host、带 `:` 的 host）、同一参数里重复端口。
2. `config`：往返序列化、缺省值、未知 `direction` 报错、重复行与同入口端口冲突各自报出两行。
3. `runtime`：记录文件名对 `a_b`、`a-b` 两种 host 稳定且不撞名；陈旧／坏 JSON 被清理。
4. `sessions`：假 ssh 立刻失败 → `Failed` 带原因 → `retry_secs` 后重试；常驻 → 过了 `up_grace` 为 `Up`；`Stop` 后 `Stopped` 且不再重启。全部使用可控时钟 + `recv_timeout`（无无界等待）。
5. `-k`：起一个假 supervisor 进程并写记录，`-k` 只杀 cmdline 匹配的那些。
6. 面板：只测纯函数（状态→文本/颜色、过滤、布局切分），不测像素。

## 13. 里程碑

1. **骨架**：`cargo new --bin ~/sl`、依赖、`forward.rs` + `config.rs` + 单测。半天。
2. **引擎**：`sessions.rs` + `runtime.rs` + 假 ssh 测试；此时 `sl <pattern>` 已能替代现有脚本。半天。
3. **面板只读**：`tui/` 骨架 + 表格 + 日志尾随 + 状态栏，数据来自引擎。半天。
4. **面板读写**：新增／编辑表单、删除与退出确认、帮助、过滤、测连。一天。
5. **收尾**：`-d` / `-k` 与面板互通、README、把 `~/.local/bin/sl` 从 zsh 脚本指向新二进制（旧脚本改名 `sl.zsh.bak` 保留一周）。半天。

合计约 3 天。

## 14. 验收清单

1. `sl` 打开面板，空配置提示“按 n 添加”；新增 `wk:8000` 后 500ms 内变 `up`，`curl localhost:8000` 通。
2. `sl -d wk:9000` 后关掉终端，`sl` 面板里看到该行（owner=daemon）；`sl -k wk` 两条都停。
3. `TMPDIR=/tmp/alt sl -d wk:9000` 之后 `sl -k` 仍能停掉（第 2 节的 bug 修复验证）。
4. 两个 Local 行抢同一 `entry`：一行 `up`，另一行 `failed` 且日志区显示 `Address already in use`。
5. 对端断开（`ssh -O exit` 或 drop 连接）：行变 `waiting` 显示倒计时，链路恢复后 5s 内回 `up`，重连次数 +1。
6. `sl wk:8000` 前台输出与今天等价（`[L] :8000 ⇒ wk:8000`），Ctrl-C 后无残留进程、无残留记录。
7. `cargo test` 全绿，且不依赖真实网络。

## 15. 风险与未验证项

- **多连接**：一行一条连接，未实测 N=20 时同 host 的表现（sshd 的 `MaxStartups` 可能拖慢首个连接）。真有这需求再换主控 + `-O forward`（已实测可用，代价是原因文本变笼统）。
- **`-R` 监听范围**：不带 bind 地址时由对端 `GatewayPorts` 决定，`GatewayPorts yes` 会绑全网卡；帮助文本与 README 必须写成条件句，不能承诺“只监听 loopback”。
- **测连 `t` 的语义**：Local 行连本地端口只能证明 ssh 在听，不代表对端端口可达；文案按“本地监听正常”措辞，不写“隧道可用”。
- **终端能力**：面板要求 ANSI + 备用屏；`TERM=dumb` 或非 tty 时直接回退到无头模式并提示。

## 16. 可逆性

- 主要不可逆面是 CLI 与配置文件：CLI 只保留现有四个入口，配置只有 `enabled` 与四个数值默认，后续只做加法。
- 会话记录是易失格式，随便改；改了就自然被“陈旧记录”逻辑清掉。
- 迁移路径：新二进制装好后把 `~/.local/bin/sl` 指向它，旧 zsh 脚本留 `sl.zsh.bak` 一周，回滚只需改回软链。

## 17. 落地记录（2026-11-18）

骨架、引擎、面板、`-d`／`-k` 与无头 CLI 已按上文实现，`cargo test` 38 项通过，`cargo clippy` 无告警。实现中与上文有出入的地方：

1. **表格列 8 列**：`标记 · 主机 · 入口 · 目标 · 状态 · 时长 · 重连 · 归属`。原计划的“方向”列并入标记（`[L]`／`[R]` 已经表达了方向），新增“归属”列，否则面板看不出某行是 `-d` 起的后台会话。
2. **弹层固定尺寸**（编辑 78×12、确认 60×10、帮助 78×22，按终端尺寸收缩），不用百分比：按比例切分在 28 行终端上会把“enter 保存”提示切掉。
3. **`-d` 的实现**：重执行自身加隐藏参数 `--daemon`（`setsid` 脱离终端），`-k` 只在进程命令行里能读到 `--daemon` 时才向 supervisor 发信号。
4. **引擎支持运行时加行**：面板新加一行时 `SetRunning(spec, true)` 对引擎里不存在的行会现场起一个监督线程。原计划只在启动时定行集合，那样面板加行得重启。
5. **新增 `runtime::reap_orphans`（原计划没有）**：面板被 SIGKILL 时它的 ssh 会变成孤儿并占着端口，下次面板启动前按“父进程是 init”把它们收掉，否则新面板全是 `Address already in use`。
6. **`-k` 删记录、留日志**：`*.log` 按行身份复用并封顶 64 KiB，留着能看上一次为什么断，删行时才连同日志一起清。
7. **计划里的 `--supervise` 隐藏参数取消**：无头前台就是“带 pattern 直接跑”，不需要第五个入口。

已实测（本机 → `wk`，OpenSSH 10.2p1）：本地／远程转发建链、`-k` 全停与按 host 停、`-d` 会话在面板里只读显示、面板加行／改行／删行、端口被占时的失败原因、链路被外部切断后 5s 内自动重连、`TMPDIR` 改掉后 `-k` 仍然能找到会话、面板退出后无残留 ssh 进程。

未验证：多 host 并发（只测了单 host 多行）、N≥10 行时的 sshd `MaxStartups` 表现、IPv6 字面量 host（仍不支持）、`GatewayPorts yes` 的对端上的 `-R` 范围。

后续可能改的点：命中“归属”是 daemon 的行时，面板目前只提示用 `sl -k` 停，不做接管；真的要接管得让 daemon 把监督权交出来，现在没这需求。
