//! 超轻量桌面分区整理工具 - 主入口
//!
//! 完整实现 Phase 0-5：
//! - WorkerW 嵌入 + 分层窗口
//! - 图标网格渲染
//! - 虚拟分组配置（文件不动，只存映射）
//! - OLE 拖放（IDropTarget + IDataObject）
//! - 多栅栏管理 + 收纳规则（sweep_rules）
//! - 禅模式 / 桌面清扫（全局热键）
//! - 开机自启
//! - 文件监听

mod config;
mod dragdrop; // OLE 拖放：手搓 COM vtable（绕过 implement 宏）
mod fence;
mod fs;
mod platform;

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

fn main() {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // 初始化 OLE（拖放依赖）
    unsafe {
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }

    // 解析桌面路径
    let desktop = std::env::var("USERPROFILE")
        .ok()
        .map(|p| PathBuf::from(p).join("Desktop"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| {
            tracing::warn!("无法定位桌面目录，使用当前目录");
            PathBuf::from(".")
        });

    tracing::info!("桌面目录: {}", desktop.display());

    // 加载或初始化配置
    let config_path = desktop.join(".fences_config.json");
    let config = Arc::new(Mutex::new(config::Config::load_or_init(
        &config_path,
        &desktop,
    )));

    // 启动时全量对账
    {
        let mut cfg = config.lock();
        cfg.reconcile(&desktop);
        let _ = cfg.save(&config_path);
    }

    // 检查并提示开机自启状态
    if platform::startup::is_startup_enabled() {
        tracing::info!("开机自启：已开启");
    } else {
        tracing::info!("开机自启：未开启（如需开启请调用 enable_startup）");
    }

    // 启动文件监听（后台线程）
    let _watcher =
        fs::watch::spawn_watcher(desktop.clone(), config.clone(), config_path.clone());

    tracing::info!("=============================");
    tracing::info!("桌面分区整理工具已启动");
    tracing::info!("热键：");
    tracing::info!("  Ctrl+Shift+H = 禅模式（隐藏/显示所有栅栏）");
    tracing::info!("  Ctrl+Shift+S = 桌面清扫（重新分类）");
    tracing::info!("  ESC = 退出");
    tracing::info!("=============================");

    // 创建并运行栅栏管理器（主线程，阻塞）
    let app = fence::FenceApp::new(config, desktop, config_path);
    app.run();
}
