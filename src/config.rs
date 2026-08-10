//! 虚拟分组配置层
//!
//! 设计：
//! - 文件物理位置不变（在 ~/Desktop 原位）
//! - 配置只存映射表：path -> fence_id + slot
//! - 启动时全量对账：扫描桌面 + 比对配置

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub mode: String,
    pub desktop_path: String,
    pub fences: Vec<Fence>,
    pub unassigned: Vec<Item>,
    #[serde(default)]
    pub sweep_rules: Vec<SweepRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fence {
    pub id: String,
    pub name: String,
    pub rect: [i32; 4], // x, y, w, h
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub path: String,    // 相对桌面根的路径
    pub slot: [u32; 2],  // [col, row]
}

/// 收纳规则：把符合 pattern 的文件自动归到指定栅栏
/// pattern 是分号分隔的通配符，如 "*.jpg;*.png;*.gif"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepRule {
    pub fence_id: String,
    pub pattern: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Config {
    /// 加载或初始化配置
    pub fn load_or_init(config_path: &Path, desktop: &Path) -> Self {
        if config_path.exists() {
            if let Ok(content) = std::fs::read_to_string(config_path) {
                if let Ok(cfg) = serde_json::from_str::<Config>(&content) {
                    tracing::info!("已加载配置: {} 个栅栏", cfg.fences.len());
                    return cfg;
                }
            }
        }

        tracing::info!("初始化默认配置");
        Config {
            version: 1,
            mode: "virtual".to_string(),
            desktop_path: desktop.to_string_lossy().to_string(),
            fences: vec![
                Fence {
                    id: "fence-main".to_string(),
                    name: "桌面文件".to_string(),
                    rect: [100, 100, 320, 240],
                    items: vec![],
                },
                Fence {
                    id: "fence-images".to_string(),
                    name: "图片".to_string(),
                    rect: [440, 100, 240, 240],
                    items: vec![],
                },
                Fence {
                    id: "fence-docs".to_string(),
                    name: "文档".to_string(),
                    rect: [100, 360, 320, 240],
                    items: vec![],
                },
            ],
            unassigned: vec![],
            sweep_rules: vec![
                SweepRule {
                    fence_id: "fence-images".to_string(),
                    pattern: "*.jpg;*.jpeg;*.png;*.gif;*.bmp;*.webp".to_string(),
                    enabled: true,
                },
                SweepRule {
                    fence_id: "fence-docs".to_string(),
                    pattern: "*.doc;*.docx;*.pdf;*.txt;*.md;*.xlsx;*.pptx".to_string(),
                    enabled: true,
                },
            ],
        }
    }

    /// 启动时全量对账
    ///
    /// 1. 扫描桌面，得到当前所有文件
    /// 2. 清理配置里失效的条目（文件已被删/移走）
    /// 3. 把桌面里有但配置里没的文件加入 unassigned
    /// 4. 应用收纳规则（sweep_rules）
    /// 5. 剩余文件放进主栅栏 fence-main
    pub fn reconcile(&mut self, desktop: &Path) {
        // 1. 扫描桌面
        let mut desktop_files: HashSet<String> = HashSet::new();
        if let Ok(entries) = std::fs::read_dir(desktop) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        // 跳过隐藏文件和配置文件
                        if !name.starts_with('.') && name != "desktop.ini" {
                            desktop_files.insert(name.to_string());
                        }
                    }
                }
            }
        }

        // 2. 清理失效条目
        for fence in &mut self.fences {
            fence.items.retain(|i| desktop_files.contains(&i.path));
        }
        self.unassigned.retain(|i| desktop_files.contains(&i.path));

        // 3. 把新文件加入 unassigned
        let mut configured: HashSet<String> = HashSet::new();
        for fence in &self.fences {
            for item in &fence.items {
                configured.insert(item.path.clone());
            }
        }
        for item in &self.unassigned {
            configured.insert(item.path.clone());
        }

        for file in &desktop_files {
            if !configured.contains(file) {
                self.unassigned.push(Item {
                    path: file.clone(),
                    slot: [0, 0],
                });
            }
        }

        // 4. 应用收纳规则
        self.apply_sweep_rules();

        // 5. 把剩余 unassigned 放进主栅栏（fence-main）
        if let Some(main_fence) = self.fences.iter_mut().find(|f| f.id == "fence-main") {
            if main_fence.items.is_empty() {
                let taken: Vec<Item> = self.unassigned.drain(..).collect();
                for (i, item) in taken.into_iter().enumerate() {
                    let col = (i as u32) % 5;
                    let row = (i as u32) / 5;
                    main_fence.items.push(Item {
                        path: item.path,
                        slot: [col, row],
                    });
                }
            }
        }

        tracing::info!(
            "对账完成: 桌面 {} 个文件, {} 个栅栏, {} 个未分类",
            desktop_files.len(),
            self.fences.len(),
            self.unassigned.len()
        );
    }

    /// 应用收纳规则：按 pattern 把 unassigned 里的文件分到对应栅栏
    pub fn apply_sweep_rules(&mut self) {
        if self.sweep_rules.is_empty() || self.unassigned.is_empty() {
            return;
        }

        let mut still_unassigned: Vec<Item> = Vec::new();
        for item in self.unassigned.drain(..) {
            let mut assigned = false;
            for rule in &self.sweep_rules {
                if !rule.enabled {
                    continue;
                }
                if pattern_matches(&rule.pattern, &item.path) {
                    if let Some(fence) = self.fences.iter_mut().find(|f| f.id == rule.fence_id) {
                        let idx = fence.items.len() as u32;
                        fence.items.push(Item {
                            path: item.path.clone(),
                            slot: [idx % 5, idx / 5],
                        });
                        assigned = true;
                        break;
                    }
                }
            }
            if !assigned {
                still_unassigned.push(item);
            }
        }
        self.unassigned = still_unassigned;
    }

    /// 把所有栅栏里的文件清空到 unassigned（用于"桌面清扫"功能）
    pub fn sweep_all(&mut self) {
        for fence in &mut self.fences {
            let drained: Vec<Item> = fence.items.drain(..).collect();
            self.unassigned.extend(drained);
        }
        // 重新应用收纳规则
        self.apply_sweep_rules();
        // 剩余的放回主栅栏
        if let Some(main_fence) = self.fences.iter_mut().find(|f| f.id == "fence-main") {
            let taken: Vec<Item> = self.unassigned.drain(..).collect();
            for (i, item) in taken.into_iter().enumerate() {
                let col = (i as u32) % 5;
                let row = (i as u32) / 5;
                main_fence.items.push(Item {
                    path: item.path,
                    slot: [col, row],
                });
            }
        }
    }

    /// 保存配置
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, content)
    }

    /// 把一个桌面文件名移动到指定栅栏（先从所有栅栏移除，再加入目标，实现"移动"语义）
    pub fn move_item_to_fence(&mut self, fence_id: &str, name: &str) {
        for fence in &mut self.fences {
            fence.items.retain(|i| i.path != name);
        }
        if let Some(fence) = self.fences.iter_mut().find(|f| f.id == fence_id) {
            let idx = fence.items.len() as u32;
            fence.items.push(Item {
                path: name.to_string(),
                slot: [idx % 5, idx / 5],
            });
        }
    }
}

/// 检查文件名是否匹配 pattern（分号分隔的通配符，如 "*.jpg;*.png"）
fn pattern_matches(pattern: &str, filename: &str) -> bool {
    let filename_lower = filename.to_lowercase();
    for pat in pattern.split(';') {
        let pat = pat.trim().to_lowercase();
        if pat.is_empty() {
            continue;
        }
        if pat == "*.*" || pat == "*" {
            return true;
        }
        if let Some(ext) = pat.strip_prefix("*.") {
            if filename_lower.ends_with(&format!(".{}", ext)) {
                return true;
            }
        } else if filename_lower == pat {
            return true;
        }
    }
    false
}
