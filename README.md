# 超轻量桌面分区整理工具 (desktop-fences)

> Fences 风格的 Rust 实现。Phase 0-5 全功能版本。
> 基于 [PROJECT_PLAN.md](../PROJECT_PLAN.md) 落地。

## 功能清单

| 功能 | 状态 | 说明 |
|---|---|---|
| WorkerW 嵌入 | ✓ | 栅栏贴桌面墙纸之上、桌面图标之下 |
| 分层窗口 | ✓ | `WS_EX_LAYERED` + `UpdateLayeredWindow` |
| 图标网格渲染 | ✓ | GDI 自绘，SHGetFileInfoW 取图标 |
| 虚拟分组配置 | ✓ | 文件不动，只存映射表（JSON） |
| 文件监听 | ✓ | `ReadDirectoryChangesW` 实时对账 |
| **OLE 拖入** | ✓ | `IDropTarget` 接收 Explorer 拖来的文件 |
| **OLE 拖出** | ✓ | `IDataObject` + `IDropSource`，从栅栏拖出到 Explorer |
| **多栅栏** | ✓ | 默认 3 个栅栏（主/图片/文档） |
| **收纳规则** | ✓ | `sweep_rules` 按扩展名自动归类 |
| **禅模式** | ✓ | Ctrl+Shift+H 一键隐藏所有栅栏 |
| **桌面清扫** | ✓ | Ctrl+Shift+S 重新分类所有文件 |
| **开机自启** | ✓ | 读写 HKCU\...\Run 注册表 |
| 翻页动画 | ✗ | 留给后续 |
| 文字阴影 | ✗ | 留给后续（需 DirectWrite） |

## 环境要求

- **Windows 10/11**
- **Rust 工具链**（stable 1.75+）
  - 安装：https://rustup.rs/
  - 默认 MSVC 工具链
- **Visual Studio Build Tools**（C++ 工作负载，含 Windows SDK）
  - 下载：https://visualstudio.microsoft.com/visual-cpp-build-tools/

## 构建与运行

```sh
cd desktop-fences
cargo run --release
```

## 热键

| 热键 | 功能 |
|---|---|
| `Ctrl+Shift+H` | 禅模式：隐藏/显示所有栅栏 |
| `Ctrl+Shift+S` | 桌面清扫：重新按规则分类所有文件 |
| `ESC` | 退出程序 |

## 交互

- **从 Explorer 拖文件到栅栏**：文件被加入栅栏（物理位置不变）
- **从栅栏拖图标到 Explorer**：复制文件（原位置不变）
- **栅栏自动刷新**：桌面文件变动后 2 秒内更新

## 默认栅栏布局

启动后会出现 3 个栅栏：

| 栅栏 | 位置 | 收纳规则 |
|---|---|---|
| 桌面文件 (fence-main) | (100, 100) 320×240 | 未匹配规则的剩余文件 |
| 图片 (fence-images) | (440, 100) 240×240 | *.jpg *.jpeg *.png *.gif *.bmp *.webp |
| 文档 (fence-docs) | (100, 360) 320×240 | *.doc *.docx *.pdf *.txt *.md *.xlsx *.pptx |

## 配置文件

运行后会在桌面生成 `.fences_config.json`：

```json
{
  "version": 1,
  "mode": "virtual",
  "desktop_path": "C:\\Users\\...\\Desktop",
  "fences": [
    {
      "id": "fence-main",
      "name": "桌面文件",
      "rect": [100, 100, 320, 240],
      "items": [{ "path": "foo.txt", "slot": [0, 0] }]
    }
  ],
  "unassigned": [],
  "sweep_rules": [
    {
      "fence_id": "fence-images",
      "pattern": "*.jpg;*.jpeg;*.png;*.gif;*.bmp;*.webp",
      "enabled": true
    }
  ]
}
```

> 文件物理位置永远在 `~/Desktop` 原位，配置只是映射表。

## 工程结构

```
desktop-fences/
├── Cargo.toml
├── README.md
└── src/
    ├── main.rs                  # 入口 + OLE 初始化
    ├── config.rs                # 虚拟分组配置 + 收纳规则 + 桌面清扫
    ├── dragdrop/
    │   ├── mod.rs               # 拖放上下文
    │   ├── target.rs            # IDropTarget（接收拖入）
    │   └── source.rs            # IDataObject + IDropSource（拖出）
    ├── fence/
    │   ├── mod.rs               # FenceApp 入口
    │   ├── manager.rs           # 多栅栏管理 + 热键 + 消息循环
    │   ├── window.rs            # 分层窗口 + WM_LBUTTONDOWN 拖出
    │   └── render.rs            # GDI 渲染
    ├── fs/
    │   ├── mod.rs
    │   ├── icon.rs              # SHGetFileInfoW
    │   └── watch.rs             # ReadDirectoryChangesW
    └── platform/
        ├── mod.rs
        ├── desktop.rs           # WorkerW 嵌入
        └── startup.rs           # 开机自启（注册表）
```

## 开机自启 API

代码里提供了注册表读写函数，可在 `main.rs` 里调用：

```rust
// 开启开机自启
platform::startup::enable_startup().ok();

// 关闭
platform::startup::disable_startup().ok();

// 查询
let on = platform::startup::is_startup_enabled();
```

当前 `main.rs` 只查询并打印状态，不自动开启——避免未经用户同意改注册表。

## 已知限制 / 可能的编译问题

代码未在沙箱内编译验证（环境无 Rust 工具链）。`windows` crate 0.58 的 API 在不同小版本可能有差异，遇到编译错误时优先检查：

1. **HWND / HICON / HINSTANCE 的 null 构造**：`HWND(std::ptr::null_mut())` vs `HWND::default()`
2. **`SendMessageTimeoutW` 返回类型**：`Result<()>` 或 `Result<usize>`
3. **`EnumWindows` 回调签名**：`ENUMWNDCALLBACK` 类型定义
4. **`BOOL` 构造**：`BOOL(0)`/`BOOL(1)` vs `BOOL(false)`/`BOOL(true)`
5. **`ReadDirectoryChangesW` 的 bytes_returned 参数**：`Option<*mut u32>` vs `*mut u32`
6. **`DragQueryFileW` 签名**：`Option<&mut [u16]>` vs `PWSTR`
7. **`implement` 宏的 IDropTarget/IDataObject 方法签名**：参数可能是值或引用，`pt: POINT` vs `&POINT`
8. **`RegOpenKeyExW` 的 hkey 参数**：`HKEY` vs `&mut HKEY`
9. **`GlobalAlloc` 返回类型**：`Result<HGLOBAL>` vs `HGLOBAL`

遇到问题查 https://docs.rs/windows/0.58/ 对应 API 的签名。

## 设计文档

完整的项目设计说明见 [../PROJECT_PLAN.md](../PROJECT_PLAN.md)，覆盖：
- 三座大山（WorkerW 嵌入、分层窗口、OLE 拖放）
- 虚拟分组 vs 真实移动的取舍
- 三层模型（物理层 / 配置层 / 视觉层）
- 阶段化实施路线

## 参考

- windows crate 文档：https://docs.rs/windows/
- Win32 API 文档：https://learn.microsoft.com/windows/win32/
- IDropTarget 文档：https://learn.microsoft.com/windows/win32/api/oleidl/nn-oleidl-idroptarget
- DoDragDrop 文档：https://learn.microsoft.com/windows/win32/api/ole2/nf-ole2-dodragdrop
