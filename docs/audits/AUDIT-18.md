# AUDIT-18 — P00~P18 全量落实审计

日期：2026-09-29（审计执行日，tag `v0.3.0` 之后）
工作区：`C:\Users\25371\projects\MinecraftServer` · main @ `d378602`
方法：承诺（TASK-INDEX / EXIT-GATES / PHASE —— **不作可解析路径引用**）→ 当前代码树 → **可运行证据**；文档默认不可信，一切数字重推。
分报告：`target/audit18/A-mechanisms.md` … `F-docs-gates.md`（工作底稿，gitignored）。
**审计与修复分离**：本文件只给 verdict / 失实清单 / 修复队列，不改产品行为。证伪探针由父代理执行并全部恢复（`git status` clean）。

---

## 0. 权重依据（历史审计空洞 + P18 新形状）

| 空洞源 | 历史证据 | 本轮权重落点 |
|---|---|---|
| 自 round-trip 绿而真机红（codec 顺序无人验证） | P18-01a patch 错序（walk 断线） | Lane C **18**：逐 codec 兄弟顺序核查 |
| 落地未接线（模块在，无 live 调用） | P18-03 ores/carvers 曾未接入 | Lane A **22**：P18 全 claim 追调用链 |
| 状态变了客户端不知道（静默不同步） | vitals 饥饿条冻结；effect skip 无 trace | Lane A/B：sync 路径覆盖 |
| 文档写 transient、实现是 permanent（缓存） | C1 亮块永不纠正 | Lane D **15**：失效完备性三条腿 |
| 测试/文档数字漂移 | AUDIT-16 F 系列、P18 三次总数同步 | Lane F **10**：逐数重推 |
| 修复≠钉住 | AUDIT-12 A12-13、17 个 owner 修 13 弱 | Lane E **13**：逐 fix 判定 STRONG/WEAK/MISSING |
| P18 walk 新代码 | b3a871c/47eb8f7/bfc05fc/16fb408 四轮 | A/B/C/D/E 全覆盖 |

P12-10 仍勿重开（P17-05 退役）。v0.3.0 已切（`9aa8c76`，CI run `36438306811` 全绿，release run `36443106811` 出 x86_64 构建）——本轮审的是 tag 树，不拦 tag。

---

## 1. 门禁与工具链证据（父代理实跑，`d378602`）

| 项 | 结果 |
|---|---|
| `python tools/gates/run.py --quick` | **every gate passed** — 1685 passed / 0 failed / 41 ignored / 135 suites |
| `cargo fmt/clippy/docs-audit` | 全绿（含于上） |
| CI（tag commit `9aa8c76`，run `36438306811`） | deny、docs-audit、x86_64、linux、aarch64 全 success |
| Release（tag 触发，run `36443106811`） | success，`mc-server-x86_64-windows.exe` + SHA256SUMS 已发布 |

---

## 2. 证伪探针（父代理实跑，中和→红/绿→恢复）

| # | 中和点 | 预期 | 实跑 | 恢复 |
|---|---|---|---|---|
| **P-1** | `KNOCKBACK_DECAY` 0.6 → 0.91 | `fold_knockback_moves_decays_and_snaps` 红 | **红**（0.4→0.24 断言） | **已恢复** |
| **P-2** | fill `volume > MAX` → `>=` | 若边界未测则**绿**（E-15） | **绿** | **已恢复** |
| **P-3** | carve/ore 顺序对调 | 若顺序无人察觉则**绿**（E-03） | **绿** | **已恢复** |
| S-1..3 | E-21/E-24/E-31 命名测试存在性 | 存在 | 存在（E-21 名有出入，见 E 备注） | n/a（只读） |

`git status` 探针后 clean；无残留。P-2/P-3 的绿是**有意证明的弱钉**，不是漏网——分别对应修复队列 P1-6（边界执行）与 P1-7（顺序 pin）。

---

## 3. 逐阶段 verdict 总表

图例：● 落实（有测试证据） · ◐ 部分落实 · ○ 未落实 · ◆ 已退化/证据丢失 · ✖ 本轮实测红

| 阶段 | 总评 | 关键结论 | 代表证据 |
|---|---|---|---|
| **P00 研究** | ● | 四仓/许可/协议基线/ADR 齐；探针工具已提交（DataComponent/Decode/Menu） | `docs/research/*`、Lane C §5 |
| **P01 基础** | ● | 工具链/lints/CI/门禁全绿（含 tag commit CI） | §1 |
| **P02 协议** | ● | ids 全断言；43/56 有 id 无单元钉（仅 ignore sweep）；17/20 未建模静默忽略 | Lane C M-1/M-6 |
| **P03 持久化** | ● | NBT/Anvil/原子保存/损坏恢复；B-02 仍 load-bearing；占位符防覆写全路径 | Lane D |
| **P04 生存竖切** | ● | 移动/交互/死亡重生/存档重启仍在 | `survival_e2e`（文件 31，矩阵写 28——F-M4） |
| **P05 模拟** | ● | 20 TPS、PHASE_ORDER、scheduled ticks | scheduler/tick_baseline |
| **P06 容器+红石早期** | ◐ | 事务守恒 ●；hopper take/generic 半写 asymmetry（B-M1）；changed flags 死文档（B-M6） | floods |
| **P07 命令/数据/生成** | ◐ | tags/loot/worldgen-first ●；配方行 1421/94 失实（F-M2）；`recipe.rs` 1516 失实（F-M6） | `vanilla_pack` pin 1454/61 |
| **P08 Pi/运维** | ◐ | save/backup/limits/RUNBOOK ●；种子不可配（D F-H1 属 P18 但根在 P08 配置面） | lifecycle/backup |
| **P09 发布/一致性** | ◐ | fixtures/差分 ●；capture 语料 56/19 仍 NOT RUN | sweep（ignore） |
| **P10 客户端兼容** | ● | 真客户端 play/light/spawn/respawn；C1 照亮补丁已合入 | goldens + walk |
| **P11 活世界** | ● | spawn/loot/战斗/掉落/持久；重连丢 XP/饱和（D F-M5，内存路径） | loot/pickup/attack |
| **P12 容器闭环** | ◐ | 窗口/熔炉/漏斗/合成 ●；**双箱逐点击写回静默 no-op（B-H1）**；同槽双 viewer LWW（B-H3） | double_chest（仅 close 路径有钉） |
| **P13 红石** | ● | tick 驱动、导电矩阵、观察者 oracle；卸载光照是 P18-C1 同类残洞（D F-M4） | differentials |
| **P14 0.2.0 可用** | ● | 管理命令/op 持久 ●；walk/screens 有据 | `admin_commands` 17 |
| **P15 可观测+加固** | ◐ | worst-phase ●；P15-03 零差仍是 determinism 自比（E-12 WEAK）；B-02 名过强（E-08） | Lane E |
| **P16 战斗生存** | ◐ | 伤害/XP/AI/挖掘/step-up ●（A 全 live 调用）；**玩家击退被丢弃而 parity 照读（A-H1）**；decay 0.6 是 feel 分叉（A-H2）；蜘蛛攀爬未建模 | Lane A |
| **P17 世界交互** | ● | AUDIT-17 P0 全关：门 facing/hinge、2×2、hopper、BE-y 均已合入且本轮复核对；trapdoor/gate e2e 偏薄（A-M8c） | doors/mechanisms/differentials |
| **P18 物品食物命令地形** | ◐ | 组件往返/磨损/饥饿/命令/矿石洞穴全有 live 调用 + walk 验收；**创造模式丢组件（B-H2）**；线缆元数据 strip（B-M5/C M-2）；出厂未知件可发 type-0（B-M3）；磁盘 healing（B-M4）；种子恒 0（D F-H1）；live determinism 未钉（D F-M1）；pack→sets 无 e2e（D F-M2） | walk + pins |

---

## 4. 分路汇总（权重 100）

| Lane | 权重 | verdict 摘要 | 一句话 |
|---|---|---|---|
| **A 机制** | 22 | 11 行：7●/3◐/1○ + C2●(fix 在)/C3◐ | 玩家击退丢失 + decay 分叉是两处 H；门已修好；P18 无孤儿调用 |
| **B 容器** | 22 | P06/P12 多 ●；P17-02 ◐；P18-01a ●线/◐盘 | **双箱写回 no-op、创造丢组件、同槽 LWW** 三处 H |
| **C 协议** | 18 | 25 域：11●/8◐/5○，0✖ | 无 H 级 wire 问题；残留是测试强度与文档真实性；patch 修复三重确认 |
| **D 持久** | 15 | 多 ●；1✖（种子恒 0） | **生产服种子恒 0** 为 H；卸载光照 + 重连丢 XP 为 M |
| **E 钉子** | 13 | 22 强（含 5 待确认）/ 10 弱 / 4 缺失 | 新钉多强；历史弱钉清单 + 每个弱/缺的中和提案 |
| **F 文档门禁** | 10 | 总数 shaders 全对；8 处漂移（1M×8 均为 M 级以下列表，最高 M） | tag 文件对树诚实，数字文档 mid-migration |

---

## 5. 最大 open gap（按严重度）

### HIGH

| ID | 位置 | 声称 | 实际 |
|---|---|---|---|
| **B-H1** | `game/mod.rs:2183` | 双箱点击写回 | 54 菜单恒 early-return：entity 首次点击即 stale，直到 close；chunk 照脏存**打开前**旧实体。close 前 crash 丢物品（菜单里见过）。仅 close 路径有钉 |
| **B-H2** | `session.rs:1304`、`menu.rs:1228` | 组件端到端 | 创造拾取/中键克隆解码后重建 plain 栈，patch 落地即丢。P18-01a 承诺在此恰失效 |
| **B-H3** | `mod.rs:2172`、`session.rs:2543` | A12-06 "closed"（P18-07） | 仅异槽修好；同槽 LWW 且零跨 viewer 推送；"closed" 失实 |
| **D F-H1** | `lifecycle.rs:220` | 种子管线（KD-28） | 生产恒 seed 0；config 无种子项；`from_level_dat` 零生产调用。开 vanilla 存档的空洞全是 seed-0 地形 |
| **A-H1** | `tick.rs:2846`、PARITY:105 | "melee knockback 0.4" | 仅 mob 受击；玩家受击注释明示丢弃。夜战"感到"的是 mob 端 + hurt 动画 |
| **A-H2** | `combat.rs:73-87` | 0.4 jar + 0.6 decay | 0.6 是 feel 分叉（vanilla ~0.91 摩擦，走四格 vs 一格）；doc 块还重复了一遍 |

### MED

| ID | 位置 | 问题 |
|---|---|---|
| D F-M4 | `world.rs:242` | chunk unload 只清自己光照，邻居 8 格 stale（C1 同类残洞）；unload 零 queue |
| D F-M5 | `session.rs:545` | 内存重连丢 XP/饱和/exhaustion（XP 清零最痛）；文件往返是全的 |
| B-M1 | `tick.rs:1305/959` | hopper take 炉先写无回滚；generic 半写注释失实（均不可达辩护） |
| B-M2 | `mod.rs:2283` | `wire_stack` 降级 plain 静默无日志（effect 食物恰触发） |
| B-M3 | `components.rs:997,1448` | 盘源 unknown 发 type-0 + 名/wire 拆成两个组件 |
| B-M4 | `components.rs:1238-1363` | `from_nbt` 悄悄 heal（附魔 map→空、range 缺省、particles 反转、saturation 双标） |
| B-M5 | `inventory.rs:139` | 元数据 ItemStack 恒 strip（掉落物渲染 plain，拾取才纠正） |
| C M-1 | c2s 43/56 | 有 id 无单元钉（仅 ignore sweep；CI 永不执行） |
| C M-5 | 9 个 S2C/config 解码器 | 尾字节宽容（测试用，无攻击面，但 sweep 论证变弱） |
| C M-6 | c2s 17/20 | 未建模静默忽略（beacon/附魔台按钮死） |
| D F-M1 | live 管线 | 同种子两次未穿过 Game 钉（顺序对调绿，P-3 实证） |
| D F-M2 | `packs.rs:310` | pack 缺失/部分失败静默 fork 地形（仅启动日志一行） |
| E WEAK 10 项 | 各命名测试 | 复合 pin、边界未执行、ignore 门控、计数差分（详见 E §1，P-2/P-3 父代理实证其中二） |
| E MISSING 4 项 | A12-07 名、081d98f 半程、respawn 臂、P17 扰动记录 | 有修无命名红测 |
| F M1–M4/M6–M8 | 矩阵/ parity/README/RELEASE | 数字 cells 滞后（逐条见 §6，全部是单格 sửa） |

---

## 6. 文档失实清单（摘，其余见分报告）

| ID | 位置 | 声称 | 实际 |
|---|---|---|---|
| F-高 | `P18-07-GATE-HEALTH.md:61` | A12-06 "closed" | 仅异槽（B-H3） |
| F-高 | PARITY `region.rs::the_location_word_is_written_last` 引用 | 该测试名 | 不存在（E §3） |
| F-高 | PARITY 伤害行 "hurt animation 2" | 事件 2 是红闪 | Round 2 已证 play 42（E §3） |
| F-中 | TEST-MATRIX area 表五格 | "measured and current" | 滞后 header（F-M1） |
| F-中 | TEST-MATRIX 差分行 1421/94 | 已被 1454/61 取代 | 同节自相矛盾（F-M2） |
| F-中 | KD-32 "12 of ~20" + README:73 | pin 证 12 | pin 证 9（`run`/嵌套被计入；F-M3） |
| F-中 | KD-22 头 "no furnace routing" | 体内已述路由 | 头体矛盾（B-D2） |
| F-中 | `mod.rs:23-26` 组件 "unmodelled" | 9 编码器已建模 | P18-01a 后失实（B-D1） |
| F-中 | wire-notes i8 窗口 / chunk-wire VarInt mask | 旧实现 | 代码已修文档未跟（C M-3/M-4） |
| F-中 | RELEASE-CANDIDATE "v0.2.0 current" | 已切 v0.3.0 | 半更新（F-M7） |
| F-中 | KD-30 "`ensure_chunk`" 接线点 | `World::ensure_chunk` 仅占位 getter | 管线在 `load_or_create_chunk`（D-D4） |
| F-中 | P18-07 "run id in the status line" | 无 status 行存在 | 证据住在别处（F-M8） |
| F-低 | verdict "tag withheld" 行 | tag 后仍在 | d378602 只改了邻行（F-L1） |
| F-低 | KD-31 time 格自相矛盾、`recipe.rs` 1516、CHANGELOG 轮数、census 脚本悬空、Game::random/remembered/save_all 三处 stale doc | 各见 F/B/D | 单句 sửa |

---

## 7. 历史 open 处置（本轮）

| 历史 ID | 处置 |
|---|---|
| AUDIT-17 P0 全 5 项 | **closed**（门/hopper/BE-y/fmt/clippy 本轮复核对） |
| A12-06/07（双 viewer/kind-drift） | 部分：异槽 + kind-drift 关；**同槽 LWW 开（B-H3）** |
| A12-03 宽度 | 仍 open（C M-1 相关：43/56 无单元钉） |
| A12-13 stale take | 仍强钉（P-2 引用；本轮未重跑） |
| B-02 命名过强 | 仍 open（E-08：timestamp/location 对调绿） |
| P15-03 零差 | 仍 WEAK（E-12：自比 determinism） |
| AUDIT-16 P1-6/P2-9/P2-10 | P2-9（unload prune）**closed**（D 确认 `tick.rs:4343`）；P1-6、P2-10 仍 open |
| P12-10 | **closed**（勿重开） |
| c2s 56/19 capture、selector differential | 仍 NOT RUN（C M-1：连单元钉都没有，不仅是缺 capture） |
| C2 挖掘失败 | fix 在（no-bump），A 评高可信；C3 蜘蛛攀爬确认未建模，死后模型无代码 referent（需抓包或转 net lane） |

---

## 8. 修复队列（审计后另开修复轮，本文件不改行为）

### P0 — 数据/主权类（建议 v0.3.1 或 P19 首批）
1. **B-H1 双箱写回**：54 菜单按半拆写（Lane B 提案 1，含测试形状）；crash 丢物是真数据丢失。
2. **D F-H1 种子**：config 加种子（默认读存档种子）+ 缺口种子测试；否则 vanilla 互操作宣称不实。
3. **B-H3 同槽 LWW + "closed" 撤回**：跨 viewer 推送或显式 LWW 文档 + P18-07 行修正。
4. **B-H2 创造组件**：拾取/克隆保留 patch（各 5 行级）+ walk-shovel 字节测试。

### P1 — 钉子与语义
5. E MISSING 4 项补命名红测；E-15 边界执行（`>=`36 行测试）；E-16 角度语义探针；E-05 拆复合 pin；E-03 顺序 pin（ores→carvers 对调红）+ pack→sets e2e（D F-M2a）。
6. C M-1：43/56 合成 round-trip + hostile 单元钉；C M-6：17/20 建模为 intent（哪怕无动作）；C M-5：9 个 lax 解码器收紧或逐个豁免；C L-3 respawn capture golden；L-4/L-5/L-6（canonicity/skip 可观测/ingress bound）。
7. A M-1..M-4/M-7/M-9/M-11/M-14 + D F-M1/M3/M4/M5/L2（A §4 25 条、B §4 10 条中可独立落地的子集；先 P0 再按下述排序：崩溃/丢物 → 静默降级 → 注释/文档）。
8. B-M1 对称提交 + 回滚；B-M2 降级日志；B-M3 type-0 拒绝 + 名匹配；B-M4 严格/Unknown 磁盘读；B-M5 元数据 strip  pin 住选择。

### P2 — 文档与历史债（多为单格 sửa，可批量）
9. F-M1/M2/M3/M4/M6/M7/M8 + F-L1..L5 + B-D1..D6 + C-D1..D4 + A-D1..D6 + D §3.1–4 + E §3（估数十行文档 edits，零行为变更；建议单 docs commit + docs-audit 即验）。
10. B-02 改名；P15-03 独立重放（仍 open）；E-12 换 pre/post 对照；c2s capture 43/56/19（C M-1 单元钉先行，不 блокирует）。

### P3 — 声明 gap（不伪装完成，P19+ 排期）
piston/rail、repeater/comparator 输出、torch delay、 brewing/村民/末地、signed chat、插件 API、附魔台/铁砧剩余效果、蜂蜜饮品、eat-effects 接线、 앉은 spider climb、metadata patch 扩展、LpVec3/MenuType 二次 oracle（C N-11）。

---

## 9. 未验证 / 边界（诚实）

| 项 | 状态 |
|---|---|
| 全量门禁 | **PASS**（§1，`d378602` 实跑 1685/0/41/135） |
| CI（tag commit） | **PASS**（run `36438306811` 五项全绿；release `36443106872` 出 x86_64 构建已发布） |
| E-21/E-24/E-31 provisional | **已关**：`a_pack_book_expands_tags_and_converts_simple_transmute`（`crafting/tests.rs:1153`，内含 `transmute_seen` 2 / `complex_transmute` 1 计数断言）、`the_speed_constant_is_the_measured_zombie_ceiling`（`mob.rs:1128`）、`wire_ids_match_the_vanilla_registry`（`effect.rs:365`）均存在——修复轮 grep + 行读确认 |
| D F-M3 极端坐标 carve | 未跑（中和提案已给） |
| capture-gated 差分 / sweep | 未跑（需 capture 环境，与历轮同） |
| Pi5 硬件 / systemd / 24h | 未在本机复验（与历轮同；P22-04 拥有） |
| 真客户端 C2/C3 复现 | 未复现（A 给出行级嫌疑；需 owner 复现细节或抓包） |
| Tp指 probe（LpVec3、MenuType order） | 未跑（C N-11，jar 工具链就绪） |

---

## 10. 一句话结论

**v0.3.0 的 tag 切在对的树上，门禁/CI/发布全绿，但审计按历史权重找到了六个 H 级和一串 M 级**：最重的是**双箱 crash 丢物（B-H1）**与**种子恒 0（D F-H1）**——前者是真数据丢失，后者让 vanilla 互操作宣称打折；其余 H（创造丢组件、同槽 LWW、玩家击退、decay 分叉）多有诚实注释或相邻 pin，修面小。P18 四个 walk finding 的修复本身多为强钉，弱处在边界与复合。文档数字在总数上诚实、在格子上 mid-migration（F-M1–M4/M7：单格 sửa 即可关）。先跑 P0 四项（建议 v0.3.1），再谈 P19。

---

*分报告：`target/audit18/{A-mechanisms,B-containers,C-protocol,D-persistence,E-pins,F-docs-gates}.md`。探针 P-1/P-2/P-3 已恢复；`git status` clean。*
