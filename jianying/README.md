# 剪映 Windows 分发

`just jianying deploy` 构建独立剪映包；Windows 的 `just deploy` 同时构建它。防抖实现和 `gyroflow_ofx.dll` 与通用 OpenFX 共用，外层只负责剪映的 ANGLE/D3D11 图像交换和已验证宿主构建的实例来源接口。不会替代达芬奇的通用 OpenFX 包。

产物是 `target/GyroflowNiyien-Jianying-windows.zip`，内含 `GyroflowJianyingBatch11.ofx.bundle`。`batch11` 文件名、插件 ID `xyz.niyien.gyroflow.jianying.batch11`、OFX 版本 1.11 以及隐藏的诊断参数保留，用于保持已有剪映草稿和参数身份；界面名称为“Gyroflow 剪映”。更新检测使用 `Contents/version.txt` 和 `Contents/package-manifest.json` 的正式发布版本，不能使用固定 OFX 版本判断更新。

包内的 `gyroflow_ofx.dll` 从同一构建的正式 OpenFX 产物复制。ANGLE 库从宿主已加载模块取得，不携带私有 ANGLE 库，不依赖用户目录、固定剪映应用版本目录或实验启动脚本。Windows 外层不再创建诊断目录或日志、抓取图像或进行输出 GPU 校验读回；桥接错误只写调试器。剪映内层仅记录警告和错误，隐藏的旧诊断参数不再改变渲染行为。

自动加载仅在 `cccreator.dll` SHA-256 精确匹配时使用本实例来源，其他构建保留浏览按钮。构建成功和离线回归不能替代宿主验收。安装器负责宿主占用检查、完整备份及失败恢复，本脚本只构建和打包，不安装。

本目录没有 macOS 发布任务。通用 OpenFX 的 macOS 包不能证明剪映 macOS 支持；基础加载、GPU、参数回写、自动加载、保存重开和签名公证须在 Mac 上分别验证。
