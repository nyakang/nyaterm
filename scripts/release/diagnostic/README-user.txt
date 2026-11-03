NyaTerm Windows x64 诊断便携版 / Diagnostic portable build

1. 将整个 ZIP 解压到一个新的、可写的目录。保留全部 EXE、PDB 和 conpty 目录。
2. 双击 NyaTerm.exe。无需安装调试器或设置 RUST_BACKTRACE。
   测试期间请勿执行在线更新，以免替换诊断 EXE。
3. 分别测试：新建本地终端资产并保存；双击已经保存的本地终端连接。
   请记录触发步骤、大致时间、Shell 类型，以及是否修改过字体和显示缩放。
4. 程序退出后，回传 data/crashes 下新生成的 panic-*.log，以及对应时段的
   data/logs/nyaterm-diagnostics.*.jsonl 和 build-info.json。
   软件中的普通诊断导出不包含 panic 报告，请单独收集 data/crashes。
5. 若默认设置不复现，再通过设置界面对照原来的字体和显示配置测试。

此包使用独立的 data 目录。不要把它覆盖到日常使用的安装目录，也不要把原来的
数据库或密钥复制到此目录。无需回传整个 data 目录、数据库、密钥或终端内容。

Extract the complete archive into a fresh writable directory and run NyaTerm.exe.
Keep the PDB files beside the executables. Reproduce both saving a local terminal
asset and connecting to an existing asset. Return new data/crashes/panic-*.log,
the corresponding data/logs/*.jsonl, build-info.json, steps and approximate time.
The ordinary diagnostic export does not include panic reports. If clean settings
do not reproduce the problem, compare font and display settings using the UI.
Do not send the database, keys, terminal contents, or the entire data directory.
