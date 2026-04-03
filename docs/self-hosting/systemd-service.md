# Vibe Kanban 系統服務安裝指南（Linux systemd）

本指南說明如何將 vibe-kanban 安裝為 systemd 系統服務、如何更新，以及如何設定 MCP Server。

---

## 1. 安裝成 systemd 系統服務

建立 service 檔案並設定固定 port 8888：

先建立 symlink（讓未來更新時不需修改 service 檔案）：

```bash
mkdir -p ~/.local/bin
ln -sf ~/.vibe-kanban/bin/v0.1.40/linux-x64/vibe-kanban ~/.local/bin/vibe-kanban-server
ln -sf ~/.vibe-kanban/bin/v0.1.40/linux-x64/vibe-kanban-mcp ~/.local/bin/vibe-kanban-mcp
ln -sf ~/.vibe-kanban/bin/v0.1.40/linux-x64/vibe-kanban-review ~/.local/bin/vibe-kanban-review
```

再建立 service 檔案：

```bash
sudo tee /etc/systemd/system/vibe-kanban.service > /dev/null <<'EOF'
[Unit]
Description=Vibe Kanban Server
After=network.target

[Service]
Type=simple
User=secudocx
ExecStart=/home/secudocx/.local/bin/vibe-kanban-server
Restart=on-failure
RestartSec=5
Environment=HOST=127.0.0.1
Environment=BACKEND_PORT=8888
Environment=VK_SHARED_API_BASE=http://localhost:8089

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable vibe-kanban
sudo systemctl start vibe-kanban
sudo systemctl status vibe-kanban
```

啟動後可透過瀏覽器開啟 `http://127.0.0.1:8888` 進入介面。

---

## 2. 未來如何更新

service 已使用 symlink 路徑（`~/.local/bin/vibe-kanban-server`），每次更新只需重建 binary 並更新 symlink，不需修改 service 檔案。

**更新流程：**

```bash
cd /home/secudocx/vibe-kanban

# 1. 拉取最新代碼
git fetch upstream
git merge upstream/main

# 2. 建置前端
cd packages/local-web && npm run build && cd ../..

# 3. 建置 Rust binaries（cargo 需在 PATH 中）
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --bin server --bin vibe-kanban-mcp --bin review

# 4. 打包並解壓到版本目錄（替換 <新版本> 為實際版本號，如 v0.1.41）
NEW_VERSION=<新版本>
INSTALL_DIR="$HOME/.vibe-kanban/bin/${NEW_VERSION}/linux-x64"
mkdir -p "$INSTALL_DIR"
cp target/release/server "$INSTALL_DIR/vibe-kanban"
cp target/release/vibe-kanban-mcp "$INSTALL_DIR/vibe-kanban-mcp"
cp target/release/review "$INSTALL_DIR/vibe-kanban-review"
chmod +x "$INSTALL_DIR"/*

# 5. 更新 symlink 指向新版本
ln -sf "$INSTALL_DIR/vibe-kanban" ~/.local/bin/vibe-kanban-server
ln -sf "$INSTALL_DIR/vibe-kanban-mcp" ~/.local/bin/vibe-kanban-mcp
ln -sf "$INSTALL_DIR/vibe-kanban-review" ~/.local/bin/vibe-kanban-review

# 6. 重啟服務
sudo systemctl restart vibe-kanban
sudo systemctl status vibe-kanban
```

---

## 3. MCP Server 設定

MCP server 透過 **stdio** 溝通，由 AI 工具（Claude Code、Cursor 等）在需要時自動啟動，不需要常駐運行。它會連接到主 server（port 8888）。

### 手動測試啟動

```bash
# 確保主 server 已在運行，再執行：
VIBE_BACKEND_URL=http://127.0.0.1:8888 \
  ~/.local/bin/vibe-kanban-mcp --mode global
```

### 在 Claude Code 中設定

編輯 `~/.claude/settings.json`，加入 MCP server 設定：

```json
{
  "mcpServers": {
    "vibe-kanban": {
      "command": "/home/secudocx/.local/bin/vibe-kanban-mcp",
      "args": ["--mode", "global"],
      "env": {
        "VIBE_BACKEND_URL": "http://127.0.0.1:8888"
      }
    }
  }
}
```

### 啟動模式說明

| 模式 | 參數 | 用途 |
|------|------|------|
| Global | `--mode global` | 預設模式，存取全域 kanban 資料 |
| Orchestrator | `--mode orchestrator` | 在 agent session 內使用，協調多個 agent |

### 環境變數

| 變數 | 說明 | 預設值 |
|------|------|--------|
| `VIBE_BACKEND_URL` | 直接指定後端完整 URL | — |
| `BACKEND_PORT` | 後端 port（不設定 VIBE_BACKEND_URL 時使用） | 從 port file 讀取 |
| `HOST` | 後端 host | `127.0.0.1` |
| `VK_SHARED_API_BASE` | 本機 remote server URL（雲端同步功能） | 不設定則停用 remote features |

---

## 快速檢查

```bash
# 查看服務狀態
sudo systemctl status vibe-kanban

# 查看即時 log
sudo journalctl -u vibe-kanban -f

# 確認 port 8888 正在監聽
ss -tlnp | grep 8888
```
