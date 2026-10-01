# AUDIT-19 — P00~P19 加权全量落实审计

日期：2026-10-01（P19 收敛之后）
工作区：`C:\Users\25371\projects\MinecraftServer` · main @ `ac19cdb`（审计基线 `e2d0c15`；见 §6 F-19-01）
方法：承诺（TASK-INDEX / EXIT-GATES / PHASE-19 —— 不作可解析路径引用）→ 当前代码树 → **可运行证据**；文档默认不可信，一切数字重推。
分报告（工作底稿，gitignored）：`target/audit19/{A-mechanisms,B-containers,C-protocol,D-persistence,E-pins,F-docs-gates,G-ops-surface}.md`，另有探针脚本与实跑 transcript 同目录。
**审计与修复分离**：本文件只给 verdict / 失实清单 / 修复队列。唯一例外是审计自己造成的 F-19-01（P19 收敛提交把提示词包文件名写进了受跟踪文档，CI 变红）——本轮已修并推送（`ac19cdb`），因为留红会污染审计自身的基线。
执行方式：审计集群 7 路并行（A/B/C/D/E/F/G），只读受跟踪文件；父代理（Lead）复核全部 H 级、实跑门禁与证伪探针。

---

## 0. 权重依据（历史审计空洞 + P19 新形状）

| 空洞源 | 历史证据 | 本轮权重落点 |
|---|---|---|
| 修复"落地"但**未接线**、或接线被判据挟持 | AUDIT-18 Lane A "无孤儿调用"；AUDIT-17 P0 队列 | **Lane A 20**：AUDIT-18 修复轮逐条追调用链 + P19 八面主链 |
| 修复≠钉住、钉子不能失败 | AUDIT-12 A12-13、AUDIT-17（14 个 owner 修 13 弱）、AUDIT-18 Lane E | **Lane E 14**：80 行 STRONG/WEAK/MISSING，逐条给中和命令 |
| 占位符/覆写类数据丢失反复出现 | Phase 05 存档被占位覆盖、AUDIT-09 B-01、**P19-08 第三次** | **Lane D 16**：全 writer 分类 + 找"第二扇门" |
| 线格式自 round-trip 绿而真机红 | P18-01a patch 错序；AUDIT-18 Lane C | **Lane C 16**：jar 字节码为准，新增 RCON / online / console 三个线面 |
| 新落地面最缺审计 | P19 五天落地 8 个切片、其中运维面首次暴露在 RCON 上 | **Lane G 12**：接入面端到端对抗 + 运维文档 |
| 容器/物品反复出 H | AUDIT-18 Lane B 三个 H | **Lane B 12**：修复复核 + 5 条 M 残项 + P19 新路径守恒 |
| 文档/数字漂移与"门禁看不见的东西" | AUDIT-16 F 系列、AUDIT-18 Lane F；本轮"文档审计绿但数字错"再次发生 | **Lane F 10**：逐数重推 + 门禁盲区 |

P12-10 仍勿重开（P17-05 退役，Lane D 复核对）。v0.3.0 tag（`9aa8c76`）不拦；P19 已于本轮收敛（`2d1d66b`，见 CHANGELOG "Phase 19 closed"）。

---

## 1. 门禁与工具链证据（父代理实跑）

| 项 | 结果 |
|---|---|
| `python tools/gates/run.py --quick`（静止树，`MC_FIXTURE_DIR` 固定） | **every gate passed** —— 1 788 passed / 0 failed / 41 ignored / 141 suites，墙钟 **1 381.8 s**（`cargo clean -p` 七个包后的全量重建；上一轮同树为 682.7 s）。fmt / clippy / 四个文档审计全绿 |
| CI（`2d1d66b`，run `36803950975`） | **success**：deny 20s · aarch64 5m29s · docs-audit 6s · x86_64 23m23s · linux 28m41s |
| CI（`e2d0c15` P19 收敛提交，run `36809539193`） | **docs-audit failure（5s）** —— 见 §6 F-19-01；其余 job 与本轮修复提交 `ac19cdb` 的 run `36810696409` 并行进行 |
| 共享 `target/` 污染（F-19-09） | 两个 lane 曾以 `CARGO_TARGET_DIR` 指向本仓库 `target/` 从影子工作树构建（Lane E 的 `target/audit19/scratch`、Lane B 的 `MinecraftServer-audit19b`），把影子路径的 `CARGO_MANIFEST_DIR` 编译进测试二进制 → `registry fixtures not found` 假红 28 例（p17_owner_pins 5、p18_commands 17、reconnect 5、seed_resolution 1）。Lane B 已 `cargo clean -p` 七个包并复跑全绿；父代理在**静止树 + 固定 fixture 目录**下重跑门禁取数。**此红是环境串台，不是产品红。** |

> 结论：本轮的"红"必须先排除两种非产品红——**目标目录串台**与**审计自己引入的文档引用**；两者都真实发生过。

---

## 2. 证伪探针（父代理实跑，中和→红/绿→恢复）

| # | 中和点 | 预期 | 实跑 | 恢复 |
|---|---|---|---|---|
| **P-1** | `session.rs:616` `player.experience = former.experience;` 删除 | 若该格无钉则应**绿** | **绿** —— `rejoin_restores_where_the_player_left` 仍 pass（测试从不断言 experience） | 已恢复，sha256 `0AC2DF60…` 前后一致 |
| **P-2** | `mod.rs:1774` `autosave_due` 恒 `false`（定时器永不触发） | 若无反空洞断言则应**绿** | **绿** —— `save_off_holds_autosave_writes_and_save_all_writes_through` 仍 pass | 已恢复，sha256 `3DDA0C5D…` 前后一致 |
| **P-3** | `commands.rs:2116` `game.load_or_create_chunk(...)` 删除（P19-08 的修复） | 两个具名红应**红** | **红**：`16 passed; 2 failed`，两钉恰为 `setblock_into_an_unloaded_chunk_keeps_the_stored_blocks`（`left: 0 / right: 5309`）与 `keep_reads_the_stored_block_of_an_unloaded_chunk`（答 `Set the block …`） | 本轮由 Lane E 在 `git archive` 影子树复跑（不动工作树）；本周父代理已在树内同法验证并字节级还原（`1293236C…`） |
| **P-4** | G-01 提权实机复现（Lane G 跑，父代理复核 transcript 与 `execute.rs:129-138`） | level-0 应被拒 | **未被拒**：`/seed` 先被拒 → `/execute as @a[name=Admin] run seed` 返回种子 → `… run op Newbie` 使其成为 level-4 → `/stop` 停服 | 只读复现，无改动 |

P-1/P-2 的**绿是证明钉子弱**（有意证明），对应修复队列 P1-5；P-3 的**红是证明钉子强**；P-4 是安全级 H 的实机证据。

---

## 3. 逐阶段 verdict 总表

图例：● 落实（有测试证据） · ◐ 部分落实 · ○ 未落实 · ◆ 已退化/证据丢失 · ✖ 本轮实测红

| 阶段 | 总评 | 关键结论 | 代表证据 |
|---|---|---|---|
| **P00 研究** | ● | 参考仓/许可/协议基线齐；jar 字节码工具链在本轮被三路复用（C 的 javap、G 的客户端探针） | Lane C/G |
| **P01 基础** | ● | 工具链/lints/CI/门禁全绿；CI 五项与 `ci.yml` 一致（docs-audit 是第六个 job） | §1 |
| **P02 协议** | ● | 18 个 id 对 jar TSV 零失配；C M-1 已由具名单元钉关闭 | Lane C |
| **P03 持久化** | ● | 原子保存链路仍真（stage→fsync→rename）；**严格磁盘读**带来向后兼容缺口（B19-4） | Lane B/D |
| **P04 生存竖切** | ● | 移动/交互/死亡重生/存档重启仍在 | `survival_e2e` |
| **P05 模拟** | ● | 20 TPS、PHASE_ORDER、scheduled ticks；D F-M4/M5 本轮关闭（含光照 pin 与重连 pin） | Lane D |
| **P06 容器+红石早期** | ◐ | B-M1 回滚仍不可达；B19-2 "loud downgrade" 实为 `debug!` | Lane B |
| **P07 命令/数据/生成** | ◐ | 命令根 39 已重推；但 level-0 默认可达 `/tp`、`/time`（G-09），`/fill` 无区块预算（A-09） | Lane A/G |
| **P08 Pi/运维** | ◐ | save/backup/limits ●；RUNBOOK §1 只装 10 张注册表里的 4 张（G-04）、§2/§5 缺整个 P19 面（G-06） | Lane G |
| **P09 发布/一致性** | ◐ | fixtures/差分 ●；发布记录 "Real-client acceptance: No" 已过期（F-19-04）；capture 语料仍 NOT RUN | Lane F |
| **P10 客户端兼容** | ◐ | 真客户端 play/light 有据；**玩家实体互不可见**（A-02：`entities::PLAYER` 全树 0 引用） | Lane A |
| **P11 活世界** | ◐ | spawn/loot/持久 ●；PvP 玩家受害路径不成立（A-01 H） | Lane A |
| **P12 容器闭环** | ◐ | 双层箱 mid-open 写回已修且钉强（B-H1 ●）；**close 臂**仍走第二套判定（B19-1，代码读） | Lane B |
| **P13 红石** | ● | 导电/观察者/tick 驱动 ●；**视图边界探针**证明红石更新既不创建也不弄脏邻区块（clobber 类无第二扇门） | Lane D |
| **P14 0.2.0 可用** | ● | 管理命令/op 持久 ● | `admin_commands` |
| **P15 可观测+加固** | ◐ | worst-phase ●；F5 日志等级仍开放（Lane C/G 复现）；三处 "no-op/deferred" 文档已过期（A-08） | Lane C/G/A |
| **P16 战斗生存** | ◐ | mob 臂 ●；**玩家受害/无敌帧路径**只写投影（A-01 H），PvP 臂被 A-02 挟持 | Lane A |
| **P17 世界交互** | ◐ | 门/观察者/漏斗 ●；跨 anchor 双箱零推送（A-03）、破坏容器只关一半 viewer（B19-1b） | Lane A/B |
| **P18 物品食物命令地形** | ◐ | 组件往返/磨损/饥饿 ●；B19-4/B19-5 两个磁盘读标准、B-M5 元数据 strip 仍无钉 | Lane B |
| **P19 访问控制运维面** | ◐ | **13 项声明实跑复现全绿**（门序、op 豁免、封禁到期、断线文案、reload 保活、写入失败回滚、1 MiB 上限、RCON 空密码拒绝、EOF 非致命、暴露告警一次、keygen 1630 ms 于绑定前）；但同层外沿有 **G-01 提权（H）**、online-mode 下 ban/whitelist uuid 失配（A-06 H）、Hello 包少一字段（C19-H1 H）、RCON 无连接上限（A-05/C19-M1）、console 一个非法字节永久失效（C19-M4） | Lane A/C/G |

---

## 4. 分路汇总（权重 100）

| Lane | 权重 | 一句话 |
|---|---|---|
| **A 机制** | 20 | AUDIT-18 P0 修复轮 B-H1/B-H2/D F-H1/A-H2 落地可达；B-H3 ◐、A-H1 ◐；P19 八面主链全 ●，但 A-01/A-06 两处 H |
| **B 容器** | 12 | B-H1/B-H2 修复真实且钉强、B-M3 关闭；**close 臂**是第二套判定（H，代码读）；5 条 M 中 B-M2/B-M5 仅注释级；P19 守恒干净 |
| **C 协议** | 16 | **C19-H1（H）：`EncryptionRequest` 少 `shouldAuthenticate`，真客户端解不出 Hello**；RCON 无预算、throttle 仅每连接、hasJoined 缺 `ip=`；C M-1 关、C M-6 建模化、C M-5 收窄到 4 个 hostile 可达解码器 |
| **D 持久** | 16 | **D-19-H1（H）：26.1.2 的种子在 `world_gen_settings.dat`，无人读 → vanilla 世界仍 seed-0（AUDIT-18 F-H1 未关）**；clobber 类无第二扇门（13 组 writer 全 read-first）；save 面 ●；D F-M4/M5 关闭 |
| **E 钉子** | 14 | 80 行判定；P19-08 两钉在影子树独立复现（值与记录一致）；WEAK：重连 XP（P-1 实证）、wire_stack 回退、save-off 反空洞（P-2 实证）、RCON 定时比较/backoff；MISSING：ban op 豁免、门序、暴露告警调用点、`/fill` 门；**A-30 纠正 AUDIT-18 的 E-08 已过期** |
| **F 文档门禁** | 10 | **F-19-01（H，本轮已修）**：P19 收敛提交写进提示词包文件名 → docs-audit 红；行数 16 vs 17、受跟踪计划索引仍写 P18、发布记录真机行过期；**数字重推全部为真**（1 788/0/41/141、39 根、8 ADR、16 审计、16 crate）；F-19-09 揭示共享 `target/` 假红 |
| **G 运维面** | 12 | **G-01（H）：`/execute as` 提权实机复现**（level-0 → op → `/stop`）；访问文件非原子写且**fail-open**（G-02/G-03）；RUNBOOK 两处缺 P19；其余 13 项 P19 声明复现绿 |

---

## 5. 最大 open gap（按严重度）

### HIGH

| ID | 位置 | 声称 | 实际 |
|---|---|---|---|
| **G-01** | `execute.rs:129-138,289-315` | 代码注释称 `as` 的权限转移"就是 Vanilla 行为"；`execute_e2e::as_does_not_grant_permission` 钉住"level-0 不能碰 console 命令" | **提权**：`select()` 把被选中玩家的 level 附到内层命令，level-0 可 `/execute as @a[name=Admin] run op <自己>` 再 `/stop`。实机 transcript 四步闭环。Vanilla 明确"execution permission level 不可被 /execute 修改"；该钉只在单 session 下成立（@a/@s/@p 全解析为调用者），**是纯空洞钉** |
| **A-01** | `session.rs:1012-1028`；`tick.rs:3008,3243-3245,3292-3293` | PARITY:105 "PvP 伤害/击退" | 玩家受击只写**投影** `entity.health`/`invulnerable_ticks`，权威 `session.player.health` 不动且不发 `SetHealth`；`tick_entity` 对 Player 早退 → **无敌帧永不衰减 → 首击后永久免疫**；推挤无 applied 判定 → 每挥都推、不掉血 |
| **D-19-H1** | `lifecycle.rs:414`；`level.rs:18` | "stored seed always wins；打开 vanilla 世界不分叉地形"（AUDIT-18 F-H1 的修复） | 真实 26.1.2 `level.dat` **没有种子键**（fixture 实测），`world_gen_settings.dat` 无人读 → `stored_seed=None` → 0；vanilla 存档的新区块全是 seed-0 地形。**该 H 未关闭，只是从"恒 0"变成"vanilla 世界恒 0"** |
| **C19-H1** | `packets/login.rs:122-128` | P19-REVIEW 第 40 行 "encryption handshake confirmed"；wire-notes "implemented+pinned"；在线模式可用 | jar `ClientboundHelloPacket(String,byte[],byte[],boolean)` 第 4 个 `shouldAuthenticate` 读在最后；我们只写 3 个字段，而我们的解码器**拒绝尾随字节**——自 round-trip 必然绿。任何真 26.1.2 客户端解 Hello 即失败 |
| **A-06** | `commands.rs:1016-1018,1046-1052`；`session.rs:479,491,1555-1563` | online 模式下 ban/whitelist 生效 | `ban_uuid_for` 恒 `offline_profile(uuid)`，而 online 门禁比 `hasJoined` 的 Mojang uuid → `/ban <在线者>` 不踢人、写出的行永不匹配，回执仍说 `Banned X`；`/whitelist add/remove <离线名>` 同理（`/op`、`/kick`、`/ban-ip` 不受影响） |
| **G-02/G-03** | `whitelist.rs:189`、`ops.rs:255`、`bans.rs:248` | "文件与列表永不失配"（写入失败回滚） | 三者都是 `std::fs::write`（O_TRUNC，无临时文件+rename）：崩在写窗口内就留截断文件，回滚来不及跑。实测截断 `banned-players.json` → "banning nobody" → **被永封玩家进服（fail-open）**；一个 `"FOREVER"` 拼写让整个封禁文件作废 |
| **B19-1** | `session.rs:1196-1233` vs `:5063` | close 也按半写回 | `double` 只看菜单长度，`flush_double_menu` 另判配对；配对在开窗期间断裂时落入单箱臂，用菜单槽号 27..53 索引 27 槽实体，被 `index < items.len()` 静默吃掉（守恒由 `drop_at_feet` 保住）。**代码读，未取得实机红——判为 HIGH 但标注 NOT VERIFIED** |
| **F-19-01** | `CHANGELOG.md:23`、`P19-REVIEW.md:100`（`e2d0c15` 引入） | "exit gate satisfied；run.py --quick 本树全绿" | 两处引用了提示词包文件名，`check_links` 判 untracked 引用 → 门禁拒绝、CI run `36809539193` docs-audit 红。**本轮已修（`ac19cdb`）**；教训：同一类错（把 gitignored 文件名写进受跟踪文档）本轮之前已犯过一次 |

### MED（摘要）

| ID | 位置 | 问题 |
|---|---|---|
| A-02 | `tick.rs:4268-4278,3589,4325`；`entities.rs:38` | 玩家投影从不入 `pending_entity_spawns`，chunk-stream 显式排除 Player → **合法客户端看不到其他玩家**；PvP 仅伪造 entity id 可达 |
| A-03 | `session.rs:3134` | 跨 viewer 推送按精确 anchor 过滤；双箱两人各开一半（R≠L）→ 零推送，对方停在开屏快照 |
| A-04 | PHASE-19:35-36 | 声称皮肤走 `player_info_update` —— 该包在树内**不存在**；P19-REVIEW 未记此项 |
| A-05/C19-M1/M2 | `apps/server/src/rcon.rs:44-60,106-130` | RCON 无连接上限、无预认证读超时、`failures` 仅每连接可重连重置；所谓 "flood" 测试**不存在**（声明超出证据） |
| C19-M3 | `online.rs:229-232` | `hasJoined` URL 缺 `ip=`，session 未绑定来源 IP（硬化损失，非绕过） |
| C19-M4 | `apps/server/src/console.rs:22-26`、`lifecycle.rs:815-817` | `read_line` 遇非法 UTF-8 返回 `Err(InvalidData)` 被并入 EOF → **一个 cp936 字节永久关闭控制台且无日志** |
| C M-5 | `config.rs:71/162/225/248` + `connection.rs:512-534` | 4 个 config c2s 解码器接受尾随字节，**确实从 socket 字节解码** → `config.rs:1-8` 的"绝不在 hostile 字节上"为假（其余 42 个已守，收窄到 4 个） |
| D-19-M1 | `config.rs:102-109` | 默认世界不记录种子；事后加 `seed=` 会分叉（与注释"cannot fork"矛盾），删掉又翻回 0 |
| A-09 | `persist.rs:74-77` vs `commands.rs:1914-1924,2115` | `/fill` 每格 `load_or_create_chunk` 无预算；最坏 1×1×32768 = **2048 区块**同步读/生成在 tick 线程（P19-08 注释已声明代价，但 `CHUNKS_PER_TICK` 承诺未覆盖命令路径） |
| A-11 | CHANGELOG:28、EXIT-GATES:117 | "白名单/封禁文件与 vanilla 互通 / vanilla 能在我们的文件上启动"：唯一证据是自 round-trip 的键集合断言，**无 vanilla 26.1.2 启动记录**，也未标 NOT RUN（对比 P18-01a 的诚实标注） |
| A-12 | `commands.rs:936-943,978-993` | `/whitelist on|remove` 不踢在线玩家，权限收回要等重连 |
| B19-1b | `tick.rs:3753` | 破坏容器只关 `open_block == Some(pos)` 的 viewer；站在幸存半边的 viewer 保留 54 槽陈旧菜单，无纠正包 |
| B19-2 | `mod.rs:2455-2465`、`session.rs:1404` | B-M2 "loud downgrade" 实为 `debug!`（默认 info 下不可见）；creative 回退臂无日志 |
| B19-4/B19-5 | `components.rs:1321-1327`、`persist.rs:686-699` | 严格磁盘读把 `saturation`/`attack_range` 变必填 → 对旧存档向后不兼容；物品栈拒绝而方块实体只 warn 后丢组件——**同一 `from_nbt` 两套标准** |
| B-M5 | `tick.rs:4345-4359` | 元数据 strip 仍是既定行为且**无具名钉**；AUDIT-18 指的位置在两个版本里都指错字段 |
| G-04/G-06 | RUNBOOK §1/§2 | §1 只装 10 张注册表里的 4 张（按 §1 装出来的服务器带 5 条 WARN、方块默认态错误、挖掘速率扁平）；§2/§5 完全没提 RCON/白名单/封禁 |
| F-19-02/F-19-03/F-19-04 | P19-REVIEW / docs/planning / RELEASE-CANDIDATE | 声明行 16 vs 实际 17；受跟踪计划索引仍写 "Current phase: P18"；发布记录"真客户端验收=No"未标注已过期 |

### LOW / INFO（摘要）

C19-L1（"vanilla 4 KiB 上限"失实，实为 ≤1460 且长度须精确；**代码与文档已按 jar 更正**，见下）、C19-L2（分块回复**经 jar 复核实为 vanilla 既有行为**：`sendCmdResponse` 按 4096 字符切分；残余是我们按字节切 4000 且无第三方客户端重组证据，已在代码注释与本文件写明）、C19-L4（accept 持续错误热旋无日志）、C19-L5（online 模块文档仍称"deferred"）、C19-L7/L8（文案用英文字面量而非可翻译键；5xx/429 一律报"Failed to verify username!"）、C19-L9/G-07（F5：失败 RCON 登录无日志，本轮再次实机复现）、G-05（缺 `block_hardness.tsv` 会连碰撞形状一起跳过，潜在）、G-08（节流预算仅每连接，约 3.0 s/连接）、G-09（level-0 默认可达 `/tp`、`/time`，vanilla 为 level 2）、G-10（`uuid` 字段不做格式校验，错字条目静默永不匹配）、G-11（console 行无上限，2 MiB 才被拒）、G-12（拒绝后的断连被记成 `socket write failed (os error 10053)`）、A-07（`default_gamemode` 装机但 join 硬编码 Survival；`/defaultgamemode` 不存在）、A-08（三处 "documented no-op / deferred" 已为 live）、A-10（写入"读不出的区块"的改动重启即失效但回执仍报成功）、D-19-L1/L2/L3（`from_level_dat` 零生产调用；skip 警告归因错误；非命令 writer 的 read-first 守卫无钉）、F-19-05..F-19-08/F-19-10（F1 行未转义竖线、会话产物计数 5 vs 6、KD-87 只是叙述端点而非行、doc-tests 4 vs 5）。

> **C19-L1/L2 的 jar 复核（本轮修复时重跑，`javap -p -c … rcon.thread.RconClient`）**：
> 请求方向 vanilla 用 `sipush 1460` 的读缓冲，并要求 `declared_length == read - 4`——**长度必须精确**，跨 read 的包直接关连接；jar 里唯一的 4096 在
> `sendCmdResponse`，且是**回复**每包 4096 **字符**的切分上限（`ByteArrayOutputStream(1248)`），所以"C19-L2 无 vanilla 先例"一句是错的，已在
> `crates/server/src/rcon.rs` 的 `VANILLA_MAX_REQUEST_BODY`/`MAX_PACKET_LEN`/`encode_response` 与 RUNBOOK 中改写；我们的 4096 是我们自己的读上限（并在 `length_accepted` 之前于分配前生效），
> 且我们**重组**跨 read 的包——这是与 vanilla 不同的健壮性选择，由 `a_body_split_across_reads_is_reassembled_not_refused` 钉住。分块回复的残余风险（按字节切分、无第三方客户端证据）写在 `encode_response` 的注释里，不再伪装成"stock reassembly"。

---

## 6. 文档失实清单（摘）

| ID | 位置 | 声称 | 实际 |
|---|---|---|---|
| F-19-01（已修） | `CHANGELOG.md:23`、`P19-REVIEW.md:100` | 阶段收敛 + 本树全绿 | 引用提示词包文件名 → docs-audit 红、CI `36809539193` 失败；`ac19cdb` 修复 |
| F-高 | `P19-REVIEW.md:40`、`wire-notes:242` | encryption handshake confirmed / pinned | 少第 4 字段（C19-H1） |
| F-高 | `CHANGELOG.md:28`、`EXIT-GATES:117` | ban/whitelist 文件与 vanilla 互通 | 仅自 round-trip（A-11） |
| F-高 | `execute.rs:129-138`（代码内注释） | `as` 的权限转移是 Vanilla 行为 | 与 Vanilla 明文相反（G-01） |
| F-高 | `PARITY:105` | PvP 伤害/击退 | 只写投影、无敌帧不衰减（A-01） |
| F-中 | `config.rs:102-109`（注释） | 设 seed 不会分叉既有世界 | 会分叉（D-19-M1） |
| F-中 | `persist.rs:74-77` | 每 tick 读取不超过 `CHUNKS_PER_TICK` | 命令路径无预算，最坏 2048 区块（A-09） |
| F-中 | `P19-REVIEW.md:51,101`、`CHANGELOG.md:111` | 16 行 claim（14 confirmed 等） | 表里 17 行、14 confirmed 需重新表述（F-19-02） |
| F-中 | `RELEASE-CANDIDATE.md:82` | "Real-client acceptance｜No" | 2026-09-30 已有真客户端 4/4（F-19-04） |
| F-中 | `docs/planning/TASK-INDEX-v0.3.0.md:3` | "Current phase: P18 … 当前阶段唯一所有者" | P19 已收敛；受跟踪的索引从未更新（F-19-03） |
| F-中 | `config.rs:1-8` | 这些解码器绝不在 hostile 字节上运行 | 4 个 config c2s 解码器确实解码 socket 字节（C M-5） |
| F-低 | `TEST-MATRIX.md:266` | doc-tests 5 | 实跑 4（F-19-10） |
| F-低 | `P19-ACCESS-SESSION.md:308-309` | 服务端日志 5 份；客户端日志每步一份 | 6 份非空；mcrcon transcript 无对应产物（F-19-06） |
| F-低 | `P19-REVIEW.md:46` | F1 行 | 未转义 `|` 导致渲染 5 列（F-19-05） |
| F-低 | `RELEASE-CANDIDATE.md:25,79,89`、`docs/README.md:34` | "KD-01…KD-87" | 87 只是叙述端点；55 行覆盖 57 个 id，1..87 中缺 23 个（F-19-07） |

**数字重推为真**（Lane F 实跑）：1 788 passed / 0 failed / 41 ignored / 141 suites；17 组 lib 计数和 1 208；命令根 39（36 literal + `msg/tell/w` 循环）；ADR 8 全 Accepted；`docs/audits/` 16 文件；16 crates + 2 binaries；CI run `36803950975` = `2d1d66b`、success、39m11s。
**门禁看不见的**：untracked 文件、数字新鲜度（只证一致）、代码/计数/ADR/KD 行、链接内容、`target/…` 反引号路径全豁免、`UNTRACKED_NAMES` 是子串匹配（分不清"引用"与"承诺"——F-19-01 正是这样进来的）。

---

## 7. 历史 open 处置（本轮）

| 历史 ID | 处置 |
|---|---|
| AUDIT-18 B-H1 双箱 mid-open 写回 | **fixed（●）**，钉强；close 臂另案（B19-1） |
| AUDIT-18 B-H2 创造组件 | **fixed（●）**，`p17_owner_pins` 5/5 + `mc-container` 136/136 |
| AUDIT-18 B-H3 同槽 LWW / "closed" | **◐**：异槽修好；跨 anchor 零推送（A-03），"closed" 仅对同 anchor 成立 |
| AUDIT-18 D F-H1 种子 | **✖ 未关闭**：vanilla 世界仍 seed-0（D-19-H1） |
| AUDIT-18 A-H1 玩家击退 | **◐**：mob 臂 ●；玩家受害/无敌帧路径不成立（A-01） |
| AUDIT-18 A-H2 decay 文档 | **fixed（●，文档级）** |
| AUDIT-18 B-M1/M2/M3/M4/M5 | B-M3 **closed**；B-M1 回滚不可达（LOW）；B-M2 仅 `debug!`；B-M4 换取严格性的兼容缺口；B-M5 **仍无钉** |
| AUDIT-18 C M-1 / M-6 / M-5 | M-1 **closed**（具名单元钉）；M-6 **closed-as-modelled**（显式 unacted）；M-5 **仍开但收窄**到 4 个 hostile 可达解码器 |
| AUDIT-18 D F-M4 / F-M5 | **both closed**（光照 unload 3×3 + queue、重连 XP 文件与内存两腿；pin 已复跑） |
| AUDIT-18 E WEAK-10 / MISSING-4 | WEAK：E-08 **已由 Lane E 纠正为 closed**（`region.rs:972` 早在 AUDIT-18 之前就断言 `location > timestamp`，AUDIT-18 该行过期）；E-03/E-05/E-15/E-16 **closed**；E-10/E-12 仍开。MISSING：A12-07 **closed**、respawn 臂 **closed-by-record**、P17 记录 **partial**、081d98f 半程 **仍开** |
| AUDIT-18 F-M1…M8 / F-L1…L5 | M1–M7 **closed**、M8 closed 有残留（`P18-07-GATE-HEALTH.md:14` 无 run id）；L1–L4 closed；L5 作为历史仍在 |
| P12-10 | **closed，勿重开**（Lane D 复核对） |
| F5（P19 收敛时具名开放） | **仍开**，且本轮由 Lane C 与 Lane G 两条独立路径实机复现（失败 RCON 登录无日志） |

---

## 8. 修复队列（审计后另开修复轮，本文件不改行为）

> **修复轮状态（2026-10-01，三波全部落地）**：本队列已清空。
> 第一波（`93619d3`）P0 1–6：G-01、A-01/A-02、D-19-H1+M1、C19-H1、G-02/G-03、A-06，外加 P1 的
> 7（RCON 预算/预认证超时/跨重连共享节流）、8（console 非法 UTF-8 + `hasJoined` 的 `ip=` 接线）、
> 9（双箱 close 臂与半边 viewer）、11（`/fill` 区块预算）、12 的 B19-1/B19-1b/A-09/A-12 与两处
> "不能失败"的钉子，以及 G-04/G-06（RUNBOOK）。
> 第二波（`a501e09`、`36f4357`）P1-12 残余与 P2：B19-2/B19-4/B19-5/B-M5、D-19-L2/L3、
> A-03/A-07/A-10、C19-L1/L2/L5/L7/L8、G-05/G-08…G-12、Lane E 三个"零钉子"行为（ban/kick 无 op
> 豁免、门序、暴露告警的**调用点**）、F-19-02…F-19-10 与 A-08。
> 第三波（`946e837`）A-04 的 feature 半边：`player_info_update`（play 70）与 `player_info_remove`
> （69）按 jar 形状实现并接入 join/leave 广播（Vanilla 的 `broadcastAll` 受众），顺带修死亡重连的
> 模式丢失、`/fill` 的先扫描后写入、`/ban` 的可选理由。
> 每一波都带具名红测 + 扰动记录 + 字节级还原；本地全闸门 **1 903 passed / 0 failed / 41 ignored /
> 153 suites**（`run.py --quick` 全绿），三波 head 的 CI 各自转绿。
> **仍具名的边界（不属本队列）**：访问文件顶层不可解析仍按 Vanilla 行为以空表启动（写入路径已不可能
> 产出这种文件）；种子文档的确切磁盘位置与键路径未验证（无 jar fixture）；`/ban-ip` 与 `/kick` 的理由
> 仍必填；`/gamemode` 变更不刷新列表条目、`UPDATE_LATENCY` 未下发；真客户端不可用，玩家列表与皮肤
> 只有"字节与 jar 及真实服务端抓包一致"这一级证据；§9 的 NOT RUN 项不变。
> 另：§7 的 **F5**（失败 RCON 登录无日志）其**实质**已由第一波的 RCON 修复覆盖——每次坏登录现在都会
> 记一条 warn，accept 持续错误也 warn+backoff；§7 表本身保留为审计当时的处置记录，不回改。

### P0 — 安全与数据（建议 v0.3.1 或 P20 首批）
1. **G-01 提权**：`/execute as|at|positioned` 不得转移权限等级——按 Vanilla 保留调用者 level（`select` 不再 `with_permission`），并按"至少两个 session"重写 `as_does_not_grant_permission`（现有钉单 session，纯空洞）。
2. **A-01 + A-02 PvP**：玩家受害走权威 `session.player.health` 并发 `SetHealth`；无敌帧在玩家路径上衰减；玩家投影进入 `pending_entity_spawns`（否则合法客户端看不到彼此，PvP 无从谈起）。
3. **D-19-H1 种子**：读 `world_gen_settings.dat`（`DimensionLayout::data_dir`）或等价物，并把解析出的种子写回 `level.dat`；补"vanilla 存档打开后不再分叉"的缺口测试。顺带 D-19-M1（默认世界记录种子）。
4. **C19-H1 Hello 包**：补 `shouldAuthenticate`（vanilla 发 `true`），并让 e2e 至少有一条"用 jar 字节序解码"的断言；同时把 P19-REVIEW/wire-notes 的 "confirmed/pinned" 降级到与证据相符。
5. **G-02/G-03 访问文件**：三个 writer 改临时文件 + fsync + rename；单行损坏不得作废整表；给"文件被截断/不可写"补 RUNBOOK 行与具名红测（fail-open 是真风险）。
6. **A-06 online 模式 uuid**：ban/whitelist 走会话内的 Mojang uuid（或 usercache），并补"online 模式下 /ban 踢人 + 行匹配"的测试；在 online 模式未修好前，EXIT-GATES 的"生效"表述必须加限定。

### P1 — 钉子与语义
7. **A-05/C19-M1/M2 RCON**：连接上限与预认证读超时（复用 `ConnectionGate`），全局/每 IP 的失败预算，补 flood 测试；或把 EXIT-GATES 的 "flood" 字样改为与证据相符。
8. **C19-M4 console**：把 `InvalidData` 与 EOF 分开（非法字节记一行、继续读），补测试；C19-M3 补 `ip=`。
9. **B19-1/B19-1b**：close 臂统一走 `flush_double_menu`（或至少让单箱臂拒绝 27..53 槽），破坏容器时关掉两个半边的 viewer；补"开窗期间配对断裂"的具名红。
10. **E 的 WEAK/MISSING 清单**：P-1/P-2 已实证的两处（重连 XP、save-off 反空洞）优先；随后 wire_stack 回退、RCON 定时比较/backoff、ban op 豁免、门序、暴露告警调用点、`/fill` 门。
11. **A-09 `/fill` 预算**：为命令路径设区块预算或明确声明代价（当前注释已写，但 `CHUNKS_PER_TICK` 的承诺要同步）。
12. **B19-2/B19-4/B19-5/B-M5/A-12/A-03/A-04/A-11**：按 §5 逐条落地；A-11 要么补 vanilla 启动记录，要么按 P18-01a 的样本标 NOT RUN。

### P2 — 文档与历史债（多为单格修改，可批量）
13. F-19-02..F-19-10（行数、受跟踪计划索引、发布记录真机行、KD 范围表述、渲染竖线、产物计数、doc-tests 数）、A-08 三处过期 no-op 文档、`config.rs:102-109` 与 `persist.rs:74-77` 的注释、G-04/G-06 的 RUNBOOK §1/§2/§5、A-07 的 `default_gamemode` scope 说明。
14. 门禁能力补强（Lane F 的盲区表）：`check_links` 应能区分"引用"与"承诺"并覆盖 untracked 文件；`check_gate_totals` 的 docstring 已自认只证一致不证新鲜。

### P3 — 声明 gap（不伪装完成）
piston/rail、repeater/comparator 输出与锁存、torch delay/burn-out、酿造/村民/末地/堡垒、signed chat、插件 API、附魔台/铁砧剩余效果、玩家互见之外的实体同步扩展、`world_gen_settings.dat` 之外的 26.1 世界格式面。

---

## 9. 未验证 / 边界（诚实）

| 项 | 状态 |
|---|---|
| 全量门禁 | 见 §1（父代理静止树实跑；`MC_FIXTURE_DIR` 固定） |
| CI（审计基线提交） | `2d1d66b` run `36803950975` success；`e2d0c15` run `36809539193` docs-audit **failure**（F-19-01，已修）；`ac19cdb` run 本轮进行中 |
| B19-1 实机红 | **未取得**（探针没落到位；Lane B 明确降级为代码读，不以推断冒充实测） |
| A-01 实机复现 | 未跑（需双 session PvP 臂；静态链已逐行给出） |
| A-02 真客户端双人抓包 | 未跑 |
| A-06 online 模式下实跑 | 未跑（无 Mojang 账号；判据是 uuid 来源不同，非猜测） |
| A-11 vanilla 启动互通 | 未跑（无 26.1.2 服务端记录） |
| `world_gen_settings.dat` 内的种子键路径 | 未验证（仓库内无该 fixture） |
| capture-gated 差分 / c2s 56/19 / selector 排序 | 仍 NOT RUN（与历轮同） |
| 跨主机 RCON、多分块回复 | 未跑（loopback bind；仅有单元钉） |
| 定时比较的实测（RCON 口令、token） | 仅结构证明，未测时 |
| Pi 5 硬件 / systemd / 24h NVMe | 未在本机复验（P22-04 拥有） |
| 共享 `target/` 串台 | 已定位（F-19-09）并在静止树重跑；**下轮审计必须为每 lane 指定独立 `CARGO_TARGET_DIR` 或串行化 cargo** |

---

## 10. 一句话结论

**P19 的门是自己关上的，而审计在同一批提交里找到七个 H：最重的是 `/execute as` 提权（G-01，实机四步闭环）、PvP 玩家受害路径根本不成立（A-01/A-02）、AUDIT-18 的种子修复对真实 vanilla 世界无效（D-19-H1：26.1 把世界生成设置挪到了 `world_gen_settings.dat`，而那个文件无人读）、以及在线模式的 Hello 包少一个字段使真客户端无法握手（C19-H1）。** P19 自己声明的 13 项访问控制行为全部复现为绿，运维面文件却是**非原子且 fail-open**（G-02/G-03），RUNBOOK 又恰好缺这一页。钉子层面：P19-08 的两钉本轮在影子树独立复现（值分毫不差），但 P-1/P-2 证明"重连 XP"与"save-off 反空洞"两处是**不能失败**的测试。文档数字大体诚实（总数、根数、ADR、审计数全部重推为真），失实集中在**声明强度**而非计数——尤其"confirmed/pinned/互通"三个词。**建议先跑 P0 六项（提权/PvP/种子/Hello/访问文件/online uuid），再谈 P20。**

---

*分报告：`target/audit19/{A-mechanisms,B-containers,C-protocol,D-persistence,E-pins,F-docs-gates,G-ops-surface}.md`；探针 P-1/P-2 已字节级还原（sha256 前后一致），P-4 只读复现；`git status` clean，无残留进程。*
