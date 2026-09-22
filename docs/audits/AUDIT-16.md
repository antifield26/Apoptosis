# AUDIT-16 — P00~P16 全量落实审计

日期：2026-09-21（审计执行日）
工作区：`C:\Users\25371\projects\MinecraftServer` · main · v0.2.0
方法：承诺（阶段提示词包 P00~P16 + TASK-INDEX + EXIT-GATES，本地 gitignored 提示词目录）→ 当前代码树 → **可运行证据**；文档默认不可信。
分报告：`target/audit16/A1..A7-*.md`（本审计工作底稿，gitignored）。
**审计与修复分离**：本文件只给 verdict / 失实清单 / 修复队列，不改产品行为。

---

## 0. 门禁与工具链证据（父代理实跑）

| 项 | 结果 |
|---|---|
| `python tools/gates/run.py --quick` | **every gate passed** · `tests: 1587 passed, 0 failed, 35 ignored, 126 suites`（含 AUDIT-16 修复新增 4 测；审计时点为 1583） |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| 端口 | 未绑 25565；未动 25567；未杀 `java.exe` |
| capture / 标签 | `target/*-capture` 与 `phase-09-final` / `v0.1.0-rc.1` / `v0.2.0` 未删未移 |

---

## 1. 证伪探针（父代理实跑，中和→红→恢复）

| # | 中和点 | 预期红测 | 实跑 | 恢复 |
|---|---|---|---|---|
| P-1 | `EffectKind::wire_id`：`id()-1` → `id()` | `wire_ids_match_the_vanilla_registry` | **红**（Speed left:1 right:0） | **绿** |
| P-2 | region 提交序：location 先于 timestamp | `location_word_is_written_after_payload` / `the_location_word_is_written_last` | **仍绿** ← **钉子弱于宣称** | 已恢复 |
| P-3 | `broadcast_hurt_animation` 空实现 | `a_landed_hit_announces_the_hurt_flash` | **红**（flash 0≠1） | **绿** |
| P-4 | `armor_absorb` 恒回 `damage` | `armor_absorb_matches_the_combat_rules_shape` | **红**（7 应余 ~1.89） | **绿** |

**P-2 发现（新）**：`location_word_is_written_after_payload` 只断言 location **在 payload 之后**；`the_location_word_is_written_last` 是终态一致性，**不**断言 location 是 journal 绝对最后一笔。调换 timestamp/location 相对顺序测试不红。崩溃安全语义（commit 在 payload 后）仍成立，但测试名 “written_last” 过强。列入修复队列 P2。

抽查实跑（非探针）：`held_damage_adds_the_weapon_bonus_over_the_fist` ok · `dig_progress` 5/5 ok · A2 持久化 21/21 ok（分报告）。

---

## 2. 逐阶段 verdict 总表

图例：● 落实（有测试证据） · ◐ 部分落实 · ○ 未落实 · ◆ 已退化/证据丢失

| 阶段 | 总评 | 关键结论 | 代表证据 |
|---|---|---|---|
| **P00 研究** | ● | 四仓/许可/协议基线 775/4790/19133/ADR 齐；provenance seed rows 仍缺；D-02 已由 ADR-0007 amend | `docs/research/*`、`docs/legal/third-party.md` |
| **P01 基础** | ● | 工具链 1.98.1、lints、CI 五门、error taxonomy、TickClock、lifecycle | `core/error.rs`、`tick.rs`、`ci.yml`；P01-12 硬化叙事并入 P15 |
| **P02 协议** | ● | framing/VarInt/hostile/login/压缩/play 管道全绿；jar 对齐 | `packet_ids` 全量；`offline_login_reaches_play…`；P-1 |
| **P03 持久化** | ● | NBT/Anvil/原子保存/损坏恢复/重启 | A2 21/21；B-02 journal 测 |
| **P04 生存竖切** | ● | 移动/交互/死亡重生/存档重启 | `survival_e2e`；`the_world_survives_a_save_and_reload` |
| **P05 模拟** | ● | 20 TPS、PHASE_ORDER、实体/AI/物理、scheduled ticks | `phase`/`scheduler`/`entity_lifecycle`/`tick_baseline` |
| **P06 容器+红石早期** | ◐ | 事务守恒/红石模型 ●；piston/rail ○；红石×容器 comparator ○；P06-18 自动化基准 ○ | `inventory_duplication`、`propagation` |
| **P07 命令/数据/生成** | ◐ | 命令/selectors/execute/function/tags/loot/worldgen-first ●；statistics ○；tag-ingredient recipe ○ | `command_e2e`、`vanilla_data`、`worldgen_e2e` |
| **P08 Pi/运维** | ◐ | save barrier/backup/limits/RUNBOOK/online fail-fast ●；Pi 硬件复验未做 | `lifecycle`、`backup`、`limits` |
| **P09 发布/一致性** | ◐ | fixtures/差分/发布脚本 ●；文档审查部分失败（见 §3）；标签只读未核 SHA | `docs/protocol/packet-ids-775.tsv` |
| **P10 客户端兼容** | ● | 真客户端进 play、light BitSet、spawn_position、respawn、entity/chat wire | 117 包解码；golden 套件 |
| **P11 活世界** | ● | 自然生成/loot 权威/双向战斗/掉落/持久；P11-03 朝向广播 ◐；P11-10 真客户端 ◐ | `natural_spawn`、`loot_and_pickup`、`player_attack`、`entity_persistence` |
| **P12 容器闭环** | ◐ | 窗口/熔炉/漏斗/合成/BE 持久 ●；P12-10 真客户端 **○ NOT RUN** | `container_e2e`、`block_entity_e2e`、`hopper_furnace` |
| **P13 红石** | ● | tick 驱动、15 格线、导电矩阵、vanilla 回放 ●；P13-08 无独立证据条 ◐ | `vanilla_conductivity`(13)、`golden_circuits` |
| **P14 0.2.0 可用** | ◐ | 管理命令/op 持久/碰撞轴序/KD-33 ●；walk/screens 依赖 owner 记录 | `admin_commands`、`ops_e2e` |
| **P15 可观测+加固** | ◐ | worst-phase ●；拆分落地但**零可观察差未独立复核**；P15-07 B-02/D-07/E-02 ●；A-03 闭集表失实 ◐ | `worst_phase_names…`；`dig_progress` 等 |
| **P16 战斗生存** | ◐ | 伤害/护甲/击退/XP 球/A* chase/骷髅/苦力怕/挖掘进度/step-up ●；**effect 数值修饰系统性缺失** ◆；AttackRange hook-only ◐；**digging/steps 屏上 NOT RUN** | `player_attack`、`xp_orbs`、`status_effects`、`mob_pathing`、`dig_progress`、`ranged_explosive` |

### P16 任务细表（本轮最深）

| ID | 承诺 | 判定 | 证据 |
|---|---|---|---|
| P16-01 | 持物伤害/护甲/伤害类型/击退/AttackRange | **●**（AttackRange ◐ bonus≡0） | `held_damage…`、`armor_absorb…`、`a_swing_shoves…`、P-4 |
| P16-02 | XP 球实体/拾取/升级/死亡散落 | **●** | `xp_orbs` 8 测；orb index 8 VarInt golden |
| P16-03 | 状态效果来源+HUD+tick | **◐** | HUD/DoT ●；**Speed/Strength/Weakness 零生产调用**（父代理 grep 证实） |
| P16-04 | A* chase/LOS/跟随范围/骷髅/苦力怕 | **●** | `mob_pathing` 5、`ranged_explosive` 5、`ai_wiring` 7 |
| P16-05 | 挖掘进度/工具速度/取消/裂纹 | **●** | `dig_progress` 5/5 实跑 |
| P16-06 | step-up 0.6 + 非全立方碰撞 | **●** | `world` step-up 系列；`block_shapes.tsv` |
| P16-07 | 真客户端夜战 | **◐** | knockback/XP/effect icon sheet 已填；**digging/steps 屏上 NOT RUN**；level-up 音效 NOT confirmed |

---

## 3. 文档失实清单（按严重度）

### HIGH

| ID | 位置 | 声称 | 实际 |
|---|---|---|---|
| F-01 | `TEST-MATRIX.md:184,216` | named suites 合计 **1 577** | 门禁 **1583**（同文件头条已写 1583） |
| F-02 | `PARITY-MATRIX.md` effect wire 段 | ride legacy table；subtract-one **reverted** | 代码 `wire_id = id()-1`（P-1 证实）；CHANGELOG/owner 屏上支持减一 |
| F-03 | `TEST-MATRIX.md:223` | survival_e2e **(27)** | **29** |
| F-04 | `TEST-MATRIX.md:223` | admin_commands **(13)** | **16** |
| F-05 | `capture_sweep.rs` | update_mob_effect **133** | jar/ids **132** |
| F-07 | `TEST-MATRIX.md:225` | inventory_duplication **(5)** | **8** |
| F-11 | `TEST-MATRIX.md:220` | protocol named 不全 | 另有 8 个 golden 套件未入表 |
| F-21 | `PARITY-MATRIX` Pathfinding 行 | 自相矛盾：“chase 已接 A*” 与 “walk ignores search / not done” 并存 | 代码 `tick.rs` `find_path` + `mob_pathing` 绕墙测 |
| F-22 | `PARITY-MATRIX` Mob AI KD-16 Gaps | “no pathfinding / walks into walls” | 同上，P16-04 已关 |
| F-23 | `PARITY-MATRIX` Commands/Data | “no selectors”；“enabled-pack not read”；命令 13/15 | `selector.rs`；`enabled.rs`；树内 **16** 条 |

### MED

| ID | 位置 | 问题 |
|---|---|---|
| F-06 | `capture_sweep` | entity_event 列为 unmodelled，实已建模 |
| F-08/09/10 | TEST-MATRIX | ai_wiring/mob_pathing/ranged 计数过时 |
| F-12 | TEST-MATRIX Server 行 | 缺 barrel/status_effects/dig_progress 等 |
| F-13/14/25 | `effect.rs` / entity `lib.rs` | “not sent update_mob_effect / no armour / future encoder” 过时 |
| F-20 | TEST-MATRIX | B-02 Gap 仍 open（P15-07 已关） |
| F-24 | `player.rs:42` | “no death drops” 与 `after_damage` 矛盾 |
| F-26 | PARITY / redstone 注释 | “target/p13-wire preserved” — 父代理核实**目录存在**（A6 误报丢失）；表述仍过强（仅 fixture 入库可复现） |
| F-27 | AUDIT-08 | P05-10/15/16 boundary 过时 |
| F-28 | region 测试名 | `…written_last` 不断言绝对最后（P-2） |

### LOW / INFO
F-15/16/17/18/19（措辞、ignored 拆解、ADR amend、RELEASE 样本）等，见 `target/audit16/A7-tests-docs.md`。

### 审计更正
- **A6 称 `target/p13-wire/` 丢失**：父代理核实 **存在**。A6 失实，F-26 降级为表述问题。
- **多路子代理称 PHASE/TASK-INDEX 不在树上**：实际在本地 gitignored 提示词目录；审计入口应指向该目录（勿写进已提交文档的可解析路径）。

---

## 4. 最大 open gap（与已知定向一致，已坐实）

1. **Effect 数值修饰系统性缺失（P16-03）** — `movement_speed_multiplier` / Strength / Weakness 在生产树 **0 调用**（仅 `effect.rs` 单元）。Resistance 与 poison/wither/regen **已接**。包与 HUD 正确，**玩法数值不变**。EXIT-GATES P16 字面可关，但相对 PHASE-16 “feel like Vanilla” 是实质缺口。
2. **P16-07 digging / steps 屏上从未跑** — 脚本有、屏上 NOT RUN；level-up 音效 NOT confirmed。
3. **P12-10 真客户端容器验收 NOT RUN**（P06/P12 遗留）。
4. **P15-03 零可观察行为差**未独立场景对照（勿用测试计数代替）。
5. **B-02 序列钉子弱于测试名**（P-2）。

---

## 5. 修复队列（按严重度，审计后另开修复轮）

### P0 — 阻断“可宣称 Vanilla 生存手感”
1. **接上 effect 数值修饰**：移动路径调 `movement_speed_multiplier`；`held_damage`/`resolve_mob_melee` 叠加 Strength/Weakness；补集成测（give speed → 位移变；give weakness → 伤害变）。
2. **修 PARITY-MATRIX 失实**：F-02 wire_id、F-21/22 pathfinding 残留、F-23 selectors/enabled-pack/命令计数。

### P1 — 测试矩阵与证据可信度
3. TEST-MATRIX：F-01 1577→1583；F-03/04/07 计数；F-08/09/10；F-11/12 补全 named 闭集；F-20 关 B-02 Gap。
4. `capture_sweep` F-05 133→132；F-06 unmodelled 表。
5. 源码过时注释：F-13/14/25、F-24、source/power/components/pack 总注（A6 R3/R4/R5/R10）。
6. **P15-03 零差复核**：可观察场景前后对照（join→dig→place→tick metrics、实体 id 序列）。

### P2 — 契约与钉子强度
7. B-02：journal 断言补 “location 是 timestamp 之后的最后一笔”，或改测试名/注释为 “after payload”。
8. 玩家死亡 drop **socket 级守恒 e2e**（A4 D6）。
9. hopper 8-tick **行为**时序测；双 viewer state-id；卸载 prune BE store。
10. MAX_ENTITIES 封顶探针去弱断言。
11. P06-18 自动化负载基准；命令计数统一 16。

### P3 — 声明 gap 跟踪（不伪装完成）
- AttackRange → P18 components；玩家击退 velocity-send；mob effects 进 chunk；spawn 实体 dirty；furnace vanilla NBT 名；piston/rail；repeater/comparator 输出向；torch delay/burn-out；tag-ingredient recipe；statistics；P12-10 / digging / steps **屏上**（owner）。

---

## 6. 未验证 / 边界（诚实）

| 项 | 状态 |
|---|---|
| 屏上 digging / steps / effect clear / poison floor | **NOT RUN**（需 owner 真客户端） |
| P12-10 容器真客户端 | **NOT RUN** |
| Level-up 音效 | NOT confirmed |
| Pi5 硬件 / systemd / SIGTERM 路径 | 未在本机复验 |
| per-crate lib 1104 逐项复测 | 未做（workspace 1583 已过） |
| 标签 SHA 只读核验 | 未做（未移动） |
| vanilla_* ignore 差分 / capture_sweep | 未跑（需 capture 环境） |
| jar/javap 独立二次 | 未重跑（沿用树内 javap 转录；关键 wire 有字节 golden） |

---

## 7. 一句话结论

**实现面整体扎实**：门禁 1583 全绿，协议/持久化/战斗/挖掘/红石核心路径有命名测试且关键修补经证伪探针钉住（wire_id、hurt flash、armor 均红/绿闭环）。  
**不可宣称“战斗手感已 Vanilla”**：effect 数值修饰未进生产、P16-07 digging/steps 与 P12-10 仍屏上空缺。  
**文档层系统性滞后**：TEST-MATRIX 计数、PARITY 矛盾句、多处“未实现/已回退/reversed”注释与代码相反——修复队列 P0/P1 应先清文档再开新功能。

---

*分报告：`target/audit16/A1-protocol.md` … `A7-tests-docs.md`。探针已全部恢复；`cargo fmt`/`clippy` 复验 clean。*
