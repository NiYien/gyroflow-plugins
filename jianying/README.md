# 剪映 Windows 分发

`just jianying deploy` 构建独立剪映包；Windows 的 `just deploy` 同时构建它。防抖实现和 `gyroflow_ofx.dll` 与通用 OpenFX 共用，外层只负责剪映的 ANGLE/D3D11 图像交换和已验证宿主构建的实例来源接口。不会替代达芬奇的通用 OpenFX 包。

产物是 `target/GyroflowNiyien-Jianying-windows.zip`，内含 `GyroflowJianyingBatch11.ofx.bundle`。Windows 导出的效果 ID 为 `xyz.niyien.jianying.gyroflow`，使剪映快速扫描得到名称“Gyroflow”；对应 PNG 图标位于 `Contents/Resources/<效果 ID>.png`。旧草稿中的 Batch11 效果需要重新添加，不再注册旧效果入口。macOS 的效果 ID 保持不变。

安装目录和二进制文件名保留，安装包 manifest 的 `plugin_id` 继续使用既有安装身份 `xyz.niyien.gyroflow.jianying.batch11`，供已发布的 Gyroflow 安装器校验；新增 `effect_id` 记录实际导出的效果 ID。更新检测仍使用 `Contents/version.txt` 和 manifest 的正式发布版本。OFX 版本 1.11 及参数身份保持不变，不能用固定 OFX 版本判断更新。

包内的 `gyroflow_ofx.dll` 从同一构建的正式 OpenFX 产物复制。ANGLE 库从宿主已加载模块取得，不携带私有 ANGLE 库，不依赖用户目录、固定剪映应用版本目录或实验启动脚本。Windows 外层不再创建诊断目录或日志、抓取图像或进行输出 GPU 校验读回；桥接错误只写调试器。剪映内层仅记录警告和错误，隐藏的旧诊断参数不再改变渲染行为。

自动加载仅在 `cccreator.dll` SHA-256 精确匹配时使用本实例来源，其他构建保留浏览按钮。构建成功和离线回归不能替代宿主验收。安装器负责宿主占用检查、完整备份及失败恢复，本脚本只构建和打包，不安装。

本目录没有 macOS 发布任务。通用 OpenFX 的 macOS 包不能证明剪映 macOS 支持；基础加载、GPU、参数回写、自动加载、保存重开和签名公证须在 Mac 上分别验证。
