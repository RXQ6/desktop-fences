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
| **隐藏原桌面图标** | ✓ | 已归入栅栏的文件从 Explorer 桌面列表移出视野，避免重复显示（best-effort，依赖 SHELLDLL_DefView；不支持时退化为普通顶层窗口） |
| **GUI 设置面板** | ✓ | Ctrl+Shift+O 打开，可视化增删栅栏 / 收纳规则，免手编 JSON |
| **移动栅栏** | ✓ | 在框内任意位置按住左键即可整体移动该框，松手后位置持久化到配置 |
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
| `Ctrl+Shift+O` | 设置面板：可视化增删栅栏与收纳规则 |
| `ESC` | 退出程序 |

## 交互

- **从 Explorer 拖文件到栅栏**：文件被加入栅栏（物理位置不变）
- **从栅栏拖图标到 Explorer**：复制文件（原位置不变）
- **拖动栅栏**：在框内任意位置按住左键即可整体移动该框（不再要求只抓标题栏）；松手后位置自动保存到配置（分层窗口位图会实时跟随，不会回弹）
- **栅栏自动刷新**：桌面文件变动后 2 秒内更新
- **`Ctrl+Shift+O` 打开设置面板**：可视化新建/删除栅栏、增删收纳规则，改动即时写入配置并应用到所有栅栏（无需手编 JSON）

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
    ├── main.rs                  # 入口 + 模块装配
    ├── config.rs                # 虚拟分组配置 + 收纳规则 + 桌面清扫
    ├── dragdrop.rs              # 手搓 OLE COM：IDropTarget / IDataObject / IDropSource
    ├── settings.rs              # GUI 设置面板（Ctrl+Shift+O，Win32 内联控件）
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
        ├── desktop.rs           # WorkerW 嵌入 + 桌面图标隐藏/恢复
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

## 编译与已知限制

已编译验证通过：**Rust 1.97.1 + WinLibs MinGW GCC (x86_64-pc-windows-gnu) + windows crate 0.58.0，0 error / 0 warning**。以下为开发过程中已在 windows 0.58 上确认、与直觉不同的 API 要点（供二次开发参考）：

1. **HWND / HICON / HINSTANCE 的 null 构造**：非 nullable 句柄用 `HWND::default()`（底层为 null），不要用 `Option<HWND>` 当 `Param<HWND>` 传参
2. **`GetModuleHandleW` 返回 `HMODULE`**：传给 `RegisterClassExW` / `CreateWindowExW` 的 `hInstance` 需 `HINSTANCE`，用 `.into()` 转换
3. **窗口样式常量类型**：`WS_*` 是 `WINDOW_STYLE`（`WS_BORDER` 等），但 `ES_*` / `LBS_*` / `BS_*` 等控件样式是裸 `i32`，混用需 `WINDOW_STYLE(bits | ES_LEFT as u32)` 包裹
4. **扩展样式不能用裸 `0`**：`CreateWindowExW` 首参必须 `WINDOW_EX_STYLE(0)`，即使为 0
5. **`GetDlgItem` / `FindWindowW` / `CreateWindowExW` 返回 `Result<HWND>`**：调用 `.is_invalid()` 或继续传参前必须先 `unwrap` / `match`，不能直接对 `Result` 操作
6. **`HMENU` 同样非 nullable**：子窗口 id 借 `hMenu` 字段时传 `HMENU(id as usize as *mut c_void)`，无菜单用 `HMENU::default()`
7. **消息框 `MessageBoxW(hWnd, ...)`**：首个参数是 `HWND`（非 `Option<HWND>`），勿包 `Some(...)`
8. **`ReadDirectoryChangesW` 的 bytes_returned 参数**：`Option<*mut u32>` vs `*mut u32`
9. **`DragQueryFileW` 签名**：`Option<&mut [u16]>` vs `PWSTR`
10. **`RegOpenKeyExW` 的 hkey 参数**：`HKEY` vs `&mut HKEY`
11. **`GlobalAlloc` 返回类型**：`Result<HGLOBAL>` vs `HGLOBAL`

遇到问题查 https://docs.rs/windows/0.58/ 对应 API 的签名。

### 桌面图标隐藏（best-effort）

隐藏已归入栅栏的桌面图标依赖 Explorer 的 `SysListView32`（位于 `SHELLDLL_DefView` 之下）。在以下情况会退化为「普通顶层窗口」且不隐藏图标：

- 无 Explorer 桌面（如无桌面的服务器 / 远程会话 / 沙箱）
- 系统 Shell 被第三方替换（如某些桌面美化工具）

该能力是体验增强，不影响「虚拟分组」核心功能——即使不隐藏，文件物理位置仍只在 `~/Desktop` 原位。

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
