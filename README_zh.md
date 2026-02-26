[English](README.md)

# FicCrawler&Lingo

![logo](assets/logo.png)

一个用 Rust 编写的命令行工具，将 [Archive of Our Own](https://archiveofourown.org) 的同人小说爬取为 **Markdown** 和 **学习用 HTML** 文件，HTML 中内置了逐段翻译/生词/搭配的填空位，专为配合大语言模型使用而设计。

## 功能特性

- 通过 URL 爬取任意 AO3 作品（支持多章节）
- 保留原文格式：**加粗**、*斜体*、~~删除线~~、标题、分割线、链接、引用、列表、表格、图片
- 提取完整元数据：分级、警告、分类、同人原作、配对关系、角色、标签、统计数据（点赞、收藏、阅读量、字数等）、摘要、作者笔记
- 生成**学习用 HTML**，每段原文下方都有翻译填空位（翻译、生词解析、常用搭配）
- 自动生成**大模型提示词模板**（`prompt.txt`），适用于翻译和词汇分析
- 超时 / 5xx / 429 错误自动重试（指数退避），重试等待期间显示实时倒计时
- 带进度条的实时状态显示
- 通过浏览器 Cookie 文件（`--cookies`）绕过 Cloudflare 限制

## 安装

```bash
git clone <本仓库>
cd EnglishReading
cargo build --release
```

编译后的二进制文件位于 `target/release/ao3-scraper`。

## 使用方法

```bash
# 基本用法 — 爬取到 ./books/<作品名>/
ao3-scraper "https://archiveofourown.org/works/27526954/chapters/67317511"

# 使用浏览器 Cookie（Cloudflare 拦截时必须）
ao3-scraper "https://archiveofourown.org/works/12345" --cookies ~/ao3_cookies.txt

# 自定义输出目录
ao3-scraper "https://archiveofourown.org/works/12345" -o my-reading

# 所有选项
ao3-scraper <URL> [选项]
  -o, --output <目录>          输出目录（默认：books）
  -d, --delay <毫秒>            请求间隔（默认：1500）
  -r, --retries <次数>          每个请求最大重试次数（默认：5）
  -t, --timeout <秒>            请求超时时间（默认：60）
      --cookies <文件>          Netscape 格式的浏览器 cookies.txt（用于绕过 Cloudflare）
```

### Cloudflare / 525 错误

AO3 使用了 Cloudflare 保护，可能会以 525 SSL 错误拦截自动化请求。解决方法是导出浏览器的 AO3 Cookie 并通过 `--cookies` 传入：

1. 在 Firefox 中安装 [cookies.txt](https://addons.mozilla.org/en-US/firefox/addon/cookies-txt/) 扩展
2. 在浏览器中登录 AO3
3. 在 AO3 页面上点击扩展图标 → **Export** → 保存为 `ao3_cookies.txt`
4. 运行：`ao3-scraper <URL> --cookies ~/ao3_cookies.txt`

> 注意：`__cf_bm` Cookie 几分钟后过期。若再次遇到 525 错误，请重新导出。

## 输出结构

```
books/
└── 作品标题/
    ├── metadata.md          # Markdown 格式的作品元数据
    ├── metadata.html        # HTML 格式的作品元数据
    ├── prompt.txt           # 大模型提示词模板
    ├── chapter1.md          # 第 1 章 Markdown
    ├── chapter1.html        # 第 1 章学习 HTML（含翻译填空位）
    ├── chapter2.md
    ├── chapter2.html
    └── ...
```

## 学习 HTML 格式说明

每个章节 HTML 文件包含：

- 简洁响应式设计，支持暗色模式
- 每个段落被包装在一个**学习块**中，包含：
  1. **原文** — 英文段落
  2. **翻译填空位** — 用于填入中文翻译
  3. **生词区域**（可折叠）— 词性、音标、释义、例句
  4. **搭配区域** — 常用词组和短语搭配

### 配合大模型使用 — 手动方式

1. 打开 `prompt.txt` — 内含即用型提示词
2. 将提示词复制到任意大模型（ChatGPT、Claude、Gemini、DeepSeek、Kimi 等）
3. 在提示词后粘贴对应的 `chapterN.md` 文件内容
4. 大模型会为每个段落输出结构化的翻译 + 生词解析 + 常用搭配
5. 将结果填入对应的 `chapterN.html` 中的空位

提示词设计为**兼容所有主流大模型**，并能产出一致的结构化输出。

### 配合 Claude API 使用 — 自动化方式（`translator/`）

`translator/` 模块通过 **Claude API** 将上述流程完全自动化（需要 Anthropic API Key）：

```bash
cd translator
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt

export ANTHROPIC_API_KEY=sk-ant-...
# 或者
export ANTHROPIC_API_KEY=$ANTHROPIC_AUTH_TOKEN

# 翻译整本书的所有章节
.venv/bin/python translate.py "../books/A Ruinous Gift"

# 只翻译指定章节
.venv/bin/python translate.py "../books/A Ruinous Gift" --chapter 1

# 从已有的 progress.json 重新渲染 HTML，不调用 API
# （手动编辑 progress.json 后使用）
.venv/bin/python translate.py "../books/A Ruinous Gift" --chapter 1 --repatch
```

- 每次 API 调用处理 15 个段落（分批处理）
- 每批完成后立即写入 `chapter{N}.progress.json`，中断后可随时续跑
- 翻译/生词/搭配直接回填到 `chapter{N}.html`，支持 Markdown 渲染（加粗、列表等转为 HTML）
- `--repatch`：跳过 API 调用，直接从 `progress.json` 重新写入 HTML，无需 API Key

详见 [`translator/README.md`](translator/README.md)。

## 翻译填空结构

HTML 中每个段落块的结构：

```
┌──────────────────────────────────┐
│  英文原文段落                      │
├──────────────────────────────────┤
│  Translation / 翻译               │  ← 填入大模型输出
│                                  │
│  ▸ Vocabulary & Chunks           │  ← 可折叠
│    word (词性) /音标/ — 释义       │
│      Example: 例句                │
│    Chunks: 短语搭配 — 含义         │
└──────────────────────────────────┘
```

## 系统要求

- Rust 1.70+（已在 1.95-nightly 上测试）
- `curl`（系统 curl，需在 PATH 中）
- 能访问 archiveofourown.org 的网络连接

## 许可证

MIT
