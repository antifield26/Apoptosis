# AUDIT-17 — P00~P17 全量落实审计

日期：2026-09-23（审计执行日）
工作区：`C:\Users\25371\projects\MinecraftServer` · main @ `571470f`
方法：承诺（本地 gitignored 提示词包：TASK-INDEX / EXIT-GATES / PHASE — **不作可解析路径引用**）→ 当前代码树 → **可运行证据**；文档默认不可信。
分报告：`target/audit17/A-mechanisms.md` … `F-docs-gates.md`（工作底稿，gitignored）+ `BRIEF.md`（权重依据）。
**审计与修复分离**：本文件只给 verdict / 失实清单 / 修复队列，不改产品行为。证伪探针由父代理执行并全部恢复。

---

## 0. 权重依据（历史审计空洞）

| 空洞源 | 历史证据 | 本轮权重落点 |
|---|---|---|
| 协议严格性 / wire 宽度符号 | A-01/02/03、AUDIT-10 四 wire、A12-01/02 | Lane C **18** |
| 容器/BE 一致性 | A12-05..10/13/14、双 viewer LWW、kind-drift | Lane B **22** + Lane D **15** |
| 测试/文档失实与计数漂移 | AUDIT-16 F-01..F-28 | Lane F **10** |
| 玩法语义 vs 宣称、facing | effect 修饰、owner A1 facing 螺丝 | Lane A **22** |
| 修复≠钉住 | AUDIT-12 A12-13 类「修了但钉弱」 | Lane E **13** |
| P17 从未被审 | 本会话 P17-01..05 + owner 14 修 | A/B/C/D/E 全覆盖 |

已关闭勿重开：**P12-10**（P17-05 owner 会话退役）。

---

## 1. 门禁与工具链证据（父代理实跑）

| 项 | 结果 |
|---|---|
| `python tools/gates/run.py --quick` | **NOT COMPLETE** — 300s / 600s 双超时，无输出（本轮**不能**写 “every gate passed”） |
| `cargo fmt --all -- --check` | **FAIL** — `session.rs` 3 处 + `tick.rs` 1 处未格式化 |
| `cargo clippy --workspace --all-targets -- -D warnings` | **FAIL** — `mc-server` **5** 个 lint（needless_pass_by_value、map_unwrap_or、collapsible_if×2、too_many_lines） |
| `cargo test -p mc-server --test doors` | **5 pass / 1 FAIL** — `placing_a_door_sets_both_oriented_halves` |
| `cargo test -p mc-server --test hopper_furnace` | **2 pass / 2 FAIL** — `hopper_above_feeds_smeltables_and_holds_fuel_back`、`a_fed_furnace_cooks_and_the_hopper_below_collects` |
| `cargo test -p mc-protocol --lib` | 123 pass / 0 fail（Lane C） |
| `cargo test -p mc-container --lib` | 135 pass / 0 fail（Lane B） |
| persistence/protocol/server 抽样合计 | 422 pass / 7 ignored（Lane D；`mc-world` lib 超时 **NOT RUN**） |
| 端口 / 标签 / capture | 未绑 25565；未动 25567；未杀 `java.exe`；标签与 `target/*-capture` 未删未移 |

**结论：`571470f` 上 fmt / clippy / doors / hopper_furnace 均为红。** 任何“门禁全绿 / P17 可标 DONE 无保留”的宣称在本提交上不成立。

---

## 2. 证伪探针（父代理实跑，中和→红/绿→恢复）

| # | 中和点 | 预期 | 实跑 | 恢复 |
|---|---|---|---|---|
| **P-1** | `place_door` facing：`player_facing` → `opposite_facing(player_facing)`（撤销 08cd8b9） | 若 08cd8b9 正确则红；若测试钉 vanilla 则 facing 断言应过 | facing 断言**过**（198 不再炸），但 **`doors.rs:203` hinge 断言红**（`left: "left" / right: "right"`） | **已恢复** |
| **P-2** | 删 `is_result_take` 上的 `&& !outcome.full_resync` | `a_stale_result_take_consumes_nothing` 红 | **红**（`left: 0 / right: 2`，网格被吃光） | **已恢复** |
| P-3（想象） | `schedule_unpress` 改 no-op | 无测变红 | Lane A F4 — 按钮粘连零钉 | n/a |
| P-4（想象） | packed_xz 交换 nibble / y 恒 0 | 无测变红 | Lane D P-D3/P-D1 — 缺 y&lt;0 / 打包钉 | n/a |

`git status` 探针后 clean；无 `FALSIFICATION` 残留。

**P-1 精化（比 Lane A 更细）**：不是“改 opposite 就绿”，而是 **facing 与 hinge 两处缺陷叠放**。改 facing 后测试推进到 hinge 行才失败——与 A17-A-02（west/north 光标轴反了）一致。

---

## 3. 逐阶段 verdict 总表

图例：● 落实（有测试证据） · ◐ 部分落实 · ○ 未落实 · ◆ 已退化/证据丢失 · ✖ 本轮实测红

| 阶段 | 总评 | 关键结论 | 代表证据 |
|---|---|---|---|
| **P00 研究** | ● | 四仓/许可/协议基线/ADR 齐；OpenSource 树在工作区（Lane A 可 symbol 对照；Lane B 误报“不在”已由 A 校正） | `docs/research/*`、`docs/legal/third-party.md` |
| **P01 基础** | ◐ | 工具链/lints/CI 存在；**本轮 fmt+clippy 红**，与 P01「quality gates operational」冲突 | §1 |
| **P02 协议** | ● | creative 56 / player_input 43 / packed_xz u8 编解码本体对齐；A-03 双侧 hard-refuse 已收敛；ids/tsv 6 测绿 | Lane C C1–C11 |
| **P03 持久化** | ● | NBT/Anvil/原子保存/损坏恢复；B-01 未读保护仍钉；B-02 journal 现钉 payload&lt;timestamp&lt;location（名仍过强） | Lane D |
| **P04 生存竖切** | ● | 移动/交互/死亡重生/存档重启仍在 | `survival_e2e`（28 `#[test]`，声称 29 失实） |
| **P05 模拟** | ● | 20 TPS、PHASE_ORDER、scheduled ticks | `phase`/`scheduler`/`tick_baseline` |
| **P06 容器+红石早期** | ◐ | 事务守恒 ●；piston/rail ○；comparator 通道 ○ | `inventory_duplication` |
| **P07 命令/数据/生成** | ◐ | tags/loot/worldgen-first ●；statistics ○ | `command_e2e`、`vanilla_data` |
| **P08 Pi/运维** | ◐ | save/backup/limits/RUNBOOK ●；Pi 硬件复验未做 | `lifecycle`、`backup` |
| **P09 发布/一致性** | ◐ | fixtures/差分 ●；GOVERNANCE「current 1384」指针过时 | Lane F |
| **P10 客户端兼容** | ● | 真客户端 play/light/spawn/respawn | golden 套件 |
| **P11 活世界** | ● | spawn/loot/战斗/掉落/持久 | `loot_and_pickup`、`player_attack` |
| **P12 容器闭环** | ◖ | 窗口/熔炉/漏斗/合成 ●；**P12-10 已退役**；A12-06/07/08/09/10/15/17/18 **八项仍开** | Lane B Historical |
| **P13 红石** | ● | tick 驱动、导电矩阵、观察者 oracle | `vanilla_observer` 3 绿、`vanilla_conductivity` |
| **P14 0.2.0 可用** | ◐ | 管理命令/op 持久 ●；walk/screens 依赖 owner | `admin_commands` 17 |
| **P15 可观测+加固** | ◐ | worst-phase ●；**P15-03 零可观察差仍无独立复核**（AUDIT-16 P1-6） | Lane F §3 |
| **P16 战斗生存** | ◐ | 伤害/XP/A*/挖掘/step-up ●；**effect 数值修饰已进生产**（AUDIT-16 P0-1 closed）；屏上 digging/steps 仍 NOT RUN | Lane F |
| **P17 世界交互** | **◐ ✖** | 机制/容器/合成主体落地；**但门 facing 红、漏斗→熔炉 2 红、2×2 关窗死代码、负 y 抹 0、owner 14 修仅 1 强钉** | §2 + 分报告 |

### P17 任务细表（本轮最深）

| ID | 承诺 | 判定 | 证据 |
|---|---|---|---|
| P17-01 | 门/观察者/发射器反应组件 | **◐ ✖** | 观察者/发射器 ●（oracle+e2e）；**门 facing 极性红测**；按钮/双开/铁门 fall-through **零钉** |
| P17-02 | 双箱/桶/漏斗→熔炉/3×3/同步/贴图/退役 P12-10 | **◐ ✖** | 双箱/桶/3×3/state_id ●；**2×2 返还死代码**；**hopper_furnace 2 红**；贴图依赖 packed_xz 但 **y&lt;0 错位** |
| P17-03 | tag 配料 + 下一类配方 | **●** | TagResolver + 简单 transmute ●；复杂/dye/imbue/smithing 诚实命名 |
| P17-04 | 观察者/发射器差分 | **●** | `vanilla_observer` 3/3 + fixture |
| P17-05 | 真客户端 build session + review | **◐** | owner sheet 已填；**P17-REVIEW 门 facing 行被 P-1/A 证伪**；14 修 13 弱钉 |

---

## 4. 分路汇总（权重 100）

| Lane | 权重 | Claims | New findings | 一句话 |
|---|---|---|---|---|
| **A 机制** | 22 | 14（1 refuted / 3 partial / 10 confirmed*） | 8（1H/4M/2L/1I） | 门 facing 极性与 Pumpkin/测试相反；按钮/双开/铁门无钉 |
| **B 容器** | 22 | 11（9 conf / 1 refuted / 1 钉住） | 5（1H/1M/2L/1I） | 2×2 关窗返还不可达；hopper feed 半写可毁物；A12 六项原样 |
| **C 协议** | 18 | 13 | 5（0H/2M/2L/1 相关） | 编解码本体对齐；**y 符号**应用侧丢；43/56 无单元钉、capture 0 体 |
| **D 持久化** | 15 | 13 | 2H/M + A12 仍开 | **负 y→0**；dropper type_id=5 应为 6；unload 不 prune BE |
| **E 钉子** | 13 | 14 owner 修 | 14（4H/7M/3L） | **N=1 强钉 / M=13 弱或无钉** |
| **F 文档门禁** | 10 | AUDIT-16 队列 + 计数 | 19（1H/8M/7L/3I） | named 闭集缺口 ~110；AUDIT-16 P0 全关、P1-6/P2-9/10 仍开 |

\* 含 code-only / partial。

---

## 5. 最大 open gap（按严重度）

### HIGH

| ID | 位置 | 声称 | 实际 |
|---|---|---|---|
| **A17-A-01 / E-01** | `session.rs:3221` · `doors.rs:198` | 门 facing=look（owner 确认「面板朝玩家」） | 测试期望 `north` when look south（即 `look.opposite()`）；HEAD **红**（got south）。Pumpkin `DoorBlock.on_place` 写 `get_horizontal_facing().opposite()`。P-1：改 opposite 后 facing 过、**hinge 仍红** |
| **A17-B-01** | `session.rs:956-958` vs `999-1002` | 2×2 关窗返还已修 | `ContainerClose` 在 window-0 **早退**，`returns_grid` 的 window-0 支是**死代码** |
| **A17-D-01 / C-01** | `tick.rs:4369` | chunk BE y 正确进包 | `u16::try_from(pos.y).unwrap_or(0)` — **y&lt;0 抹成 0**；Pumpkin 为 `i16` |
| **Gates** | 树根 | 门禁全绿 | **fmt FAIL · clippy FAIL（5）· doors 1 红 · hopper_furnace 2 红** |
| **A17-E 系列** | owner 14 修 | 「修了且验证」 | **仅 state_id 强钉**；creative/双开/按钮/潜行 bit5 等 **7 项零测试** |

### MED

| ID | 位置 | 问题 |
|---|---|---|
| A17-A-02 | `session.rs:3651-3661` | hinge 光标 west/north 与 Pumpkin 相反（P-1 坐实叠放） |
| A17-B-02 | `tick.rs:1181-1196` | hopper→furnace 先写 hopper 再写 furnace，半写毁物 |
| A17-B-03 | `tick.rs:1197` | feed 不标 furnace dirty |
| A17-D-02 | `tick.rs:4360-4366` | dropper type_id 写死 5（dispenser），应为 6 |
| A17-D-03 | `tick.rs:4202-4212` | unload 不 prune `BlockEntityStore`（A12 泄漏） |
| A17-D-04 | `mod.rs:2112-2125` | 双 viewer 整表 LWW（A12-06） |
| A17-C-04 | — | creative 56 / player_input 43 **无单元钉** |
| A17-C-05 | capture corpus | c2s 对 43/56/19 **0 体**；sweep 对这些 NOT covered |
| A17-E-11 | `two_nearby_stacks_merge…` | 同 age 双堆，`>=`/`<=` 中和均绿 |
| A17-F-01 | `TEST-MATRIX.md:184` | named 闭集缺口 **~110**（protocol goldens 42 + P16 e2e 23 + P17 e2e 29 + 杂） |
| A17-F-02 | `TEST-MATRIX.md:223` | survival_e2e 声称 29、源码 **28** |
| A17-F-03 | `TEST-MATRIX.md:176` vs `:225` | mc-container 134 vs 135 内讧（实测 135） |

---

## 6. 文档失实清单（摘）

| ID | 位置 | 声称 | 实际 |
|---|---|---|---|
| F-高 | `docs/testing/P17-REVIEW.md:23` | 「Door panel toward the player = confirmed」 | **refuted**（P-1 + 红测 + Pumpkin） |
| F-高 | 本地提示词包 EXIT-GATES §P17 Status | SATISFIED | 在 doors/hopper_furnace 红、2×2 死代码下**过强**；应降为 conditional 或重开 |
| F-高 | 本地提示词包 TASK-INDEX P17-01..05 全 DONE | 与上同 | 同上 |
| F-中 | `protocol/.../mod.rs:1044-1048` + `chunk.rs:362,643` | packed_xz 为 u16 /「各 16-bit」 | 字段已是 u8；三处 doc 自相矛盾 |
| F-中 | `survival_e2e.rs:1035-1036` | noop 点击不 bump state | 7753e03 起 **always bump**（A17-B-04） |
| F-中 | `hopper_furnace.rs:170-171` | hopper 朝向 unmodelled | ebf1f19 已建模 |
| F-中 | `doors.rs:202` | hinge “lands left” | 断言 `"right"` |
| F-中 | `TEST-MATRIX.md:249` | vanilla_pack 1421/94 | 实为 1454/61（P17-03） |
| F-低 | `GOVERNANCE-REPORT.md` | current 1384 指针 | 过时（现 1587 链） |

AUDIT-16 修复队列核销（Lane F）：**P0 全关**（effect 修饰 + PARITY 四反转）；**P1-3/5 partial**；**P1-6 / P2-9 / P2-10 仍 open**；P12-10 已退役。

---

## 7. 历史 open 处置（本轮）

| 历史 ID | 处置 |
|---|---|
| A-01/02/03 | 编解码与 trailing 约定 **confirmed closed**（Lane C） |
| A12-01 wire | **closed**；应用层 `window_id:u8` 残留（A17-C-03） |
| A12-02 close u8→VarInt | **仍钉住** |
| A12-03 宽度残留 | **仍 open** |
| A12-06/07/08/09/10/15/17/18 | **八项全部仍 open**（P17 未覆盖） |
| A12-13 stale take | **仍强钉**（P-2 红证实） |
| B-01 | holds |
| B-02 | journal 已钉 timestamp 序；测试名 `written_last` 仍过强 |
| AUDIT-16 P0 effect 修饰 | **closed** |
| AUDIT-16 P1-6 P15-03 零差 | **仍 open** |
| AUDIT-16 P2-9 unload prune | **仍 open** |
| AUDIT-16 P2-10 MAX_ENTITIES | **仍 open** |
| **门 facing=look** | **REOPENED**（A17-A-01） |
| P12-10 | **closed**（P17-05；勿重开） |

---

## 8. 修复队列（审计后另开修复轮）

### P0 — 阻断「P17 可宣称完成」
1. **门 facing 极性 + hinge 轴**：对照 jar/`DoorBlock.getStateForPlacement`/`getHinge` 一次做对；改 `place_door`/`door_hinge`；让 `placing_a_door_sets_both_oriented_halves` 不改期望变绿（或改期望前先有 jar 证据）。同步重写 `session.rs:3215-3220` 错误前提注释。
2. **2×2 关窗返还**：在 window-0 早退**前** `return_craft_grid`；补 e2e。
3. **hopper_furnace 2 红**：先修到绿（与 A17-B-02 半写一并），再补扰动证明。
4. **chunk BE y 符号**：`i16`（或 `pos.y as u16` 位型）+ 越界拒绝；补 y=-60 wire 断言。
5. **fmt + clippy 清零**（5 lint + 4 处格式）。

### P1 — 钉子与语义
6. owner 14 修补钉（Lane E「预期红测」名单 14 条）：优先 creative 解码、双开门、按钮脉冲、潜行 bit5。
7. dropper type_id=6；hopper feed 原子写 + furnace dirty。
8. packed_xz/y 文档三处对齐；`survival_e2e` / `hopper_furnace` 过时注释。
9. TEST-MATRIX：named 闭集补 42+23+29；29→28；134/135 统一；1421/94→1454/61。
10. 本地提示词包 EXIT-GATES §P17 / TASK-INDEX 状态降级或加条件。

### P2 — 契约与历史债
11. A12-06 双 viewer 版本号；A12-07 kind-drift；unload prune BE（P2-9）。
12. hopper 8-tick 行为时序测；MAX_ENTITIES 强钉（P2-10）；P15-03 零差场景对照（P1-6）。
13. capture 语料补 43/56/19；A12-03 宽度校验。
14. B-02 测试名 `written_last` → `written_after_payload_and_timestamp` 或钉 journal 末笔。

### P3 — 声明 gap（不伪装完成）
piston/rail；repeater/comparator 输出向；torch delay；crafting_dye/imbue/smithing/special_*；transmute 组件拷贝；AttackRange；玩家击退 velocity-send；熔炉 vanilla NBT 名；fence-gate swing 条件（A17-A-06 需 oracle）；屏上 digging/steps。

---

## 9. 未验证 / 边界（诚实）

| 项 | 状态 |
|---|---|
| `tools/gates/run.py --quick` 全量 | **NOT COMPLETE**（双超时） |
| `mc-world` lib 全量 | **NOT RUN**（Lane D 超时） |
| 真客户端门 hinge / 2×2 / y&lt;0 贴图 / hopper 路由屏上 | **NOT RUN** |
| jar `javap` 独立二次 | 未重跑（Pumpkin + 树内转录；Lane A 有 symbol 级对照） |
| vanilla_* ignore 差分 / capture_sweep | 未跑（需 capture 环境） |
| Pi5 / systemd / SIGTERM | 未在本机复验 |
| per-crate lib 1104 逐项 | 未做 |
| Lane B「OpenSource 不在工作区」 | **审计更正**：树在 `OpenSourceMinecraftServer/`（Lane A 已用）；B 的二手引述限制仅影响 transmute 对照 |

---

## 10. 一句话结论

**实现面比 P17 声明的薄一层皮厚很多，但出口门不能按 SATISFIED 无保留签字**：协议/容器/红石核心多数有证据，然而 **门 facing 与漏斗→熔炉在 main 上是红测**、**2×2 关窗还是死代码**、**地下容器 y 被抹成 0**、**owner 14 修 13 条无强钉**，且 **fmt/clippy 当场红**。  
AUDIT-16 的 P0（effect 修饰、PARITY 反转句）与 P12-10 确已关闭；A12 容器历史空洞**零关闭**。  
先跑修复队列 P0，再重签 P17 / 开 P18。

---

*分报告：`target/audit17/{A-mechanisms,B-containers,C-protocol,D-persistence,E-owner-pins,F-docs-gates}.md`。探针 P-1/P-2 已恢复；`git status` clean。*
