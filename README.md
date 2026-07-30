# Hermes VoiceDesk

> 实时语音对话桌面应用 — Hermes Agent 的语音交互终端

[![Rust](https://img.shields.io/badge/rust-1.97.1-orange.svg)](https://www.rust-lang.org)
[![Tauri](https://img.shields.io/badge/tauri-2.11-blue.svg)](https://v2.tauri.app)
[![Vue](https://img.shields.io/badge/vue-3.4-green.svg)](https://vuejs.org)

## 概述

Hermes VoiceDesk 是一个基于 Tauri v2 的桌面窗口应用，提供与 Hermes Agent 的实时语音对话功能。作为 [shujietai（枢界台）](https://github.com/guancyxx/shujietai)的功能延伸，VoiceDesk 专注于语音交互体验。

### 功能特性

- 🎤 **实时语音对话** — VAD 检测 + STT 转写 + Hermes LLM + TTS 朗读
- 🔊 **打断支持 (Barge-in)** — 说话即打断 AI，自然对话体验
- 📋 **系统托盘常驻** — 菜单栏图标，随时唤醒
- ⌨️ **全局热键** — Option+Space 一键呼出/隐藏
- 🌊 **音频可视化** — 实时波形显示
- 🔧 **工具调用展示** — 实时显示 Hermes 的工具调用过程
- 📝 **文本输入 fallback** — 不方便说话时可直接打字

### 技术栈

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri v2 (Rust) |
| 前端 | Vue 3 + TypeScript + Vite |
| 音频采集 | cpal + coreaudio (Rust) |
| VAD | silero-vad ONNX (Rust) |
| STT | macOS NSSpeechRecognizer / faster-whisper |
| TTS | macOS AVSpeechSynthesizer / Edge TTS |
| AI 后端 | Hermes Agent API (localhost:8642) |
| 通信 | HTTP POST /v1/runs + SSE streaming |

### 架构

```
┌────────────────────────────────────────┐
│            Tauri Desktop App            │
│  ┌──────────────┐  ┌─────────────────┐ │
│  │  Vue 3 UI    │  │   Rust Backend   │ │
│  │  - Waveform  │  │  - Audio Capture │ │
│  │  - Transcript│  │  - VAD Engine    │ │
│  │  - Response  │  │  - STT Engine    │ │
│  │              │  │  - Hermes Client │ │
│  └──────────────┘  │  - TTS Engine    │ │
│                     │  - System Tray   │ │
│                     └────────┬────────┘ │
└──────────────────────────────┼──────────┘
                               │
                     ┌─────────▼─────────┐
                     │  Hermes API       │
                     │  localhost:8642   │
                     │  /v1/runs (SSE)   │
                     └───────────────────┘
```

## 快速开始

### 前置要求

- Rust 1.77+ (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Node.js 20+ + npm
- macOS 14+ (当前仅支持 macOS)
- Hermes Agent (API server 运行中)

### 开发

```bash
# 安装依赖
npm install

# 启动 Hermes API server (另一个终端)
API_SERVER_ENABLED=true API_SERVER_KEY=shujietai-dev-key-2026 hermes gateway run

# 启动开发模式
npm run tauri dev
```

### 构建

```bash
npm run tauri build
# 输出: src-tauri/target/release/bundle/
```

## 项目结构

```
hermes-voicedesk/
├── src/                        # Vue 3 前端
│   ├── views/VoiceChat.vue     # 主对话界面
│   ├── components/
│   │   ├── AudioWave.vue       # 音频波形
│   │   ├── Transcription.vue   # 输入框/转写
│   │   ├── ResponseCard.vue    # AI 响应
│   │   └── StateIndicator.vue  # 状态指示
│   └── router/
├── src-tauri/                  # Rust 后端
│   ├── src/
│   │   ├── lib.rs              # 入口：tray、hotkey、commands
│   │   ├── audio/              # 音频模块
│   │   │   ├── capture.rs      # 麦克风采集
│   │   │   ├── vad.rs          # VAD 检测
│   │   │   └── player.rs       # TTS 播放
│   │   ├── stt/                # 语音识别
│   │   │   ├── apple.rs        # macOS 系统 STT
│   │   │   └── whisper.rs      # faster-whisper
│   │   ├── api/
│   │   │   └── hermes.rs       # Hermes API 客户端
│   │   └── session/
│   │       └── store.rs        # 对话历史存储
│   ├── Cargo.toml
│   └── tauri.conf.json
├── ARCHITECTURE.md             # 完整架构文档
└── package.json
```

## 实施阶段

- [x] Phase 0: 项目初始化 + 架构搭建
- [ ] Phase 1: MVP — 音频采集 + VAD + STT + Hermes SSE + TTS
- [ ] Phase 2: 打断机制 + 流式 TTS + 对话历史 + 设置
- [ ] Phase 3: shujietai 集成
- [ ] Phase 4: 多语言 + 离线模式 + 跨平台

## 相关项目

- [shujietai](https://github.com/guancyxx/shujietai) — AI Agent 调度中枢
- [Hermes Agent](https://github.com/NousResearch/hermes-agent) — AI Agent 框架
- 架构文档：`voice-desk-research/ARCHITECTURE.md`

## License

MIT
