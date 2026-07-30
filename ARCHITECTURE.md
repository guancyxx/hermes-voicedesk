# Hermes VoiceDesk — Architecture & Technical Design

> 实时语音对话桌面窗口应用，作为 shujietai（枢界台）的功能延伸。
> 可与 Hermes Agent 进行自然语音对话，支持打断、系统托盘常驻、全局快捷键唤醒。

---

## 1. 项目定位

### 1.1 与 shujietai 的关系

```
┌─────────────────────────────────────────────────────┐
│                   shujietai (枢界台)                  │
│  Web Dashboard — Task Board — Dispatch — 多项目管理  │
│  Vue 3 + FastAPI + PostgreSQL + Redis               │
│  角色：AI Agent 调度中枢、任务管理、项目视角            │
└──────────────────────────┬──────────────────────────┘
                           │ 共享 Hermes API
                           │ 可选：任务创建/状态同步
┌──────────────────────────┴──────────────────────────┐
│               Hermes VoiceDesk (新项目)               │
│  桌面窗口应用 — 实时语音对话 — 系统托盘 — 全局热键     │
│  Tauri v2 + Vue 3 + Rust                            │
│  角色：语音交互终端、快速问答、随手任务                  │
└─────────────────────────────────────────────────────┘
```

**决策：独立 GitHub repo**
- 理由：不同的构建工具链（Tauri/Rust vs Docker/FastAPI），不同的发布形态（桌面 app vs Web 服务）
- 共享方式：通过 Hermes API 通信，可选通过 shujietai dispatch API 同步任务
- 仓库名建议：`hermes-voicedesk`

---

## 2. 技术栈选型

### 2.1 桌面框架对比

| 维度 | Tauri v2 | Electron | Swift Native |
|------|----------|----------|--------------|
| 包体积 | ~5-10 MB | ~150 MB | ~10-15 MB |
| 内存占用 | ~50 MB | ~150 MB | ~30 MB |
| 启动速度 | 快 (<1s) | 慢 (2-4s) | 极快 (<0.5s) |
| 音频延迟 | 低 (Rust 原生) | 中 (Node.js 层) | 极低 (AVAudioEngine) |
| 系统托盘 | ✅ tauri-plugin-tray | ✅ electron-tray | ✅ NSStatusBar |
| 全局热键 | ✅ tauri-plugin-global-shortcut | ✅ globalShortcut | ✅ CGEvent |
| Web Audio API | ✅ WebView 内 | ✅ Chromium | ❌ 需 WKWebView |
| 麦克风权限 | ✅ 系统级弹窗 | ✅ 系统级弹窗 | ✅ AVAudioSession |
| 跨平台 | ✅ Win/Mac/Linux | ✅ Win/Mac/Linux | ❌ macOS only |
| 开发体验 | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ |
| 学习曲线 | Rust 基础 | JS 全栈 | Swift + AppKit |
| 维护成本 | 中 | 低 | 高 |
| 社区生态 | 快速增长 | 极其成熟 | Apple 独占 |

**推荐：Tauri v2**

理由：
1. 与现有 Vue 3 技术栈完美契合（前端代码几乎可直接复用）
2. 包体积小、内存低，适合常驻系统托盘的场景
3. Rust 后端可直接调用系统音频 API，延迟低
4. 跨平台能力（未来可扩展到 Windows/Linux）
5. Tauri v2 已稳定，插件生态成熟（tray、global-shortcut、notification 等）

### 2.2 STT（语音识别）方案

| 方案 | 延迟 | 精度 | 离线 | 费用 | macOS 适配 |
|------|------|------|------|------|------------|
| **faster-whisper** | 低（本地 GPU） | 极高 | ✅ | 免费 | ⭐⭐⭐⭐ |
| macOS 内置听写 | 极低 | 高 | ✅ | 免费 | ⭐⭐⭐⭐⭐ |
| Web Speech API | 中 | 中 | ❌ | 免费 | ⭐⭐⭐ |
| Groq Whisper | 极低 | 极高 | ❌ | 免费额度 | ⭐⭐⭐⭐ |
| OpenAI Whisper | 低 | 极高 | ❌ | $0.006/min | ⭐⭐⭐⭐ |

**推荐：macOS 内置听写（NSSpeechRecognizer）+ faster-whisper 作为 fallback**

理由：
- macOS 内置听写延迟最低（系统级优化，M4 Max Neural Engine 加速）
- faster-whisper 在 M4 Max 上运行极快（GPU 加速），支持离线场景
- 不需要额外的 API 费用
- 可用 Hermes 已有的 STT 配置（local faster-whisper）

**实现路径**：
- Tauri Rust 端调用 `NSSpeechRecognizer`（通过 `objc` crate 或 `tauri-plugin-speech`）
- 备选：通过 Python subprocess 调用 faster-whisper（复用 Hermes 已安装的）
- Hermes API server 已集成 STT，可通过 API 提交音频获得文本

### 2.3 TTS（文本转语音）方案

| 方案 | 延迟 | 自然度 | 离线 | 费用 | 流式 |
|------|------|--------|------|------|------|
| macOS AVSpeechSynthesizer | 极低 | 中 | ✅ | 免费 | ✅ |
| Edge TTS | 低 | 高 | ❌ | 免费 | ❌ |
| ElevenLabs | 中 | 极高 | ❌ | $ | ✅ |
| OpenAI TTS | 低 | 极高 | ❌ | $$ | ✅ |
| Web SpeechSynthesis | 低 | 中 | ✅ | 免费 | ✅ |

**推荐：macOS AVSpeechSynthesizer（系统 TTS）+ Edge TTS fallback**

理由：
- 系统 TTS 零延迟、零费用、离线可用
- Edge TTS 提供更自然的音色作为可选增强
- Hermes 已有 Edge TTS 配置
- 不需要额外 API

### 2.4 VAD（语音活动检测）

**推荐：silero-vad**

| 方案 | 精度 | 延迟 | 大小 |
|------|------|------|------|
| **silero-vad** | 极高 | <1ms | ~2MB ONNX |
| WebRTC VAD | 中 | <1ms | ~50KB |
| rnnoise | 中（降噪+VAD） | <1ms | ~100KB |

**实现路径**：
- Rust 端用 `ort` (ONNX Runtime) crate 加载 silero-vad 模型
- 或在 WebView 端用 `onnxruntime-web` 运行
- 推荐 Rust 端运行，延迟更低、不阻塞 UI 线程

---

## 3. 系统架构

### 3.1 整体架构图

```
┌──────────────────────────────────────────────────────────────┐
│                     Tauri Desktop App                         │
│                                                              │
│  ┌──────────────────────┐    ┌──────────────────────────┐    │
│  │    Vue 3 Frontend     │    │     Rust Backend          │    │
│  │  ┌──────────────────┐ │    │  ┌──────────────────────┐ │    │
│  │  │ Audio Visualizer │ │    │  │ Audio Capture        │ │    │
│  │  │ (Web Audio API)  │ │    │  │ (cpal + coreaudio)   │ │    │
│  │  └──────────────────┘ │    │  └────────┬─────────────┘ │    │
│  │  ┌──────────────────┐ │    │           │               │    │
│  │  │ Transcription    │ │    │  ┌────────▼─────────────┐ │    │
│  │  │ Display          │ │    │  │ VAD Engine           │ │    │
│  │  └──────────────────┘ │    │  │ (silero-vad ONNX)    │ │    │
│  │  ┌──────────────────┐ │    │  └────────┬─────────────┘ │    │
│  │  │ Response Stream  │ │    │           │               │    │
│  │  │ (Markdown)       │ │    │  ┌────────▼─────────────┐ │    │
│  │  └──────────────────┘ │    │  │ STT Engine           │ │    │
│  │  ┌──────────────────┐ │    │  │ (NSSpeechRecognizer  │ │    │
│  │  │ State Machine    │ │    │  │  / faster-whisper)   │ │    │
│  │  │ (Pinia Store)    │ │    │  └────────┬─────────────┘ │    │
│  │  └──────────────────┘ │    │           │               │    │
│  └──────────────────────┘    │  ┌────────▼─────────────┐ │    │
│                              │  │ Hermes API Client    │ │    │
│                              │  │ (SSE streaming)      │ │    │
│                              │  └────────┬─────────────┘ │    │
│                              │           │               │    │
│                              │  ┌────────▼─────────────┐ │    │
│                              │  │ TTS Engine           │ │    │
│                              │  │ (AVSpeechSynthesizer)│ │    │
│                              │  └──────────────────────┘ │    │
│                              │  ┌──────────────────────┐ │    │
│                              │  │ System Tray          │ │    │
│                              │  │ Global Hotkey        │ │    │
│                              │  └──────────────────────┘ │    │
│                              └──────────────────────────┘    │
└──────────────────┬───────────────────────────────────────────┘
                   │
     ┌─────────────┴─────────────┐
     │                           │
┌────▼────────┐          ┌──────▼────────┐
│ Hermes API  │          │ shujietai API │
│ :8642       │          │ :18000        │
│             │          │ (可选集成)      │
│ /v1/runs    │          │ /api/v1/      │
│ SSE events  │          │ task-board    │
│ STT / TTS   │          │ dispatch      │
└─────────────┘          └───────────────┘
```

### 3.2 数据流（一次语音对话的完整生命周期）

```
1. [用户说话]
   Mic → Audio Buffer (环形缓冲, 16kHz/16bit/mono)

2. [VAD 检测]
   每 30ms 帧送入 silero-vad → 检测 speech_start / speech_end
   speech_start: 开始累积音频
   speech_end:   触发 ASR

3. [STT 转写]
   累积的音频 → NSSpeechRecognizer / faster-whisper → 文本

4. [LLM 推理]
   文本 → Hermes API POST /v1/runs
        → SSE GET /v1/runs/{run_id}/events
        → 流式接收 message.delta / tool.started / tool.completed

5. [TTS 朗读]
   LLM 响应文本 → AVSpeechSynthesizer.speak()
   支持打断: 检测到用户再次说话 → 立即停止 TTS

6. [UI 更新]
   全程通过 Tauri events (emit/listen) 同步前后端状态
   前端展示: 波形动画 → 转写文本 → AI 响应（流式打字效果）
```

### 3.3 状态机设计

```
                    ┌──────────────────────────────────┐
                    │                                  │
                    ▼                                  │
              ┌──────────┐    VAD:speech_start    ┌────────┐
   app start  │  IDLE    │ ──────────────────────→│LISTENING│
   ─────────→ │ (待机)    │                        │ (收音)  │
              └──────────┘                        └───┬────┘
                    ▲                                 │
                    │                    VAD:speech_end│
                    │                                 ▼
                    │                          ┌──────────┐
                    │         TTS done         │THINKING  │
                    │    ┌─────────────────────│ (思考中)  │
                    │    │                     └─────┬────┘
                    │    │              SSE:first_delta│
               ┌────┴────┴─┐                          ▼
               │ SPEAKING  │                    ┌──────────┐
               │ (朗读中)   │←───────────────────│RESPONDING│
               └─────┬─────┘  TTS 开始播放       │ (响应中)  │
                     │                           └──────────┘
                     │
            VAD:speech_start
            (用户打断) → 停止 TTS → 回到 LISTENING
```

### 3.4 打断（Barge-in）机制

```rust
// Rust 端核心逻辑
struct BargeInManager {
    tts_handle: Option<TtsHandle>,    // 可中断的 TTS 句柄
    vad_sensitivity: f32,             // VAD 灵敏度（说话时降低）
    cooldown_ms: u64,                 // AI 开始说话后的短暂冷却期
}

impl BargeInManager {
    fn on_tts_start(&mut self) {
        // AI 开始说话后 500ms 内不触发打断（避免自激）
        self.cooldown_ms = 500;
    }

    fn on_user_speech_detected(&mut self) -> bool {
        if self.cooldown_ms > 0 {
            return false; // 冷却期内忽略
        }
        if let Some(handle) = self.tts_handle.take() {
            handle.stop();           // 立即停止 TTS
            return true;             // 触发打断
        }
        false
    }
}
```

### 3.5 句子边界流式策略（Sentence-Boundary Streaming）

区别于简单按字数切分或等所有 tokens 到齐再 TTS，最佳实践是按**句子边界**触发 TTS：

```
LLM SSE Token Stream: "今天" "天气" "不错" "。" "适合" "出门" "。"

累积缓冲区:  "今天天气不错。"
              ↑ 检测到句号 → 立即触发 TTS: "今天天气不错。"

累积缓冲区:            "适合出门。"
                       ↑ 检测到句号 → 触发 TTS: "适合出门。"
```

| 策略 | 首字延迟 | 自然度 | 实现复杂度 |
|------|----------|--------|-----------|
| 逐字 TTS | 极低 | 差（机械） | 低 |
| **按句边界 TTS** | 低 | **高** | 中 |
| 等全文 TTS | 高 | 最高 | 低 |

**触发标点**：`。！？. ! ? \n\n`

### 3.6 始终在听的 VAD（Always-On VAD during Playback）

传统的 push-to-talk 需要用户按键，体验差。VoiceDesk 支持"始终在听"模式：

```
┌─────────────────────────────────────────────┐
│  TTS 播放期间                                │
│  麦克风持续采集 → VAD 检测（灵敏度降低 30%）   │
│  检测到用户说话 → 立即停止 TTS → 切换到聆听    │
│  500ms 冷却期 → 防止 AI 自己的声音触发 VAD    │
└─────────────────────────────────────────────┘
```

---

## 4. 与 Hermes API 集成

### 4.1 Hermes API 已有能力

从 shujietai 的 `hermes_connector.py` 分析，Hermes API server 提供：

```
POST /v1/runs              启动 Agent run
  Body: { input, conversation_history?, instructions?, model?, session_id? }
  Response: 202 { run_id }

GET /v1/runs/{run_id}/events    SSE 事件流
  Events:
    message.delta     → { delta: "文本增量" }
    tool.started      → { tool: "工具名", arguments: {...} }
    tool.completed    → { tool: "工具名", duration, error }
    reasoning.available → { text: "推理过程" }
    run.completed     → { usage: {...} }
    run.failed        → { error: "..." }
```

### 4.2 VoiceDesk 如何使用

VoiceDesk 直接使用 Hermes API，不需要经过 shujietai 后端：

```typescript
// 前端调用（通过 Tauri invoke 转到 Rust 端）
const response = await invoke('hermes_chat', {
  message: "你好，今天有什么新闻？",
  sessionId: currentSessionId,  // 保持会话连续性
});

// Rust 端实现
async fn hermes_chat(message: String, session_id: Option<String>) -> Result<Stream> {
    // 1. POST /v1/runs 启动
    let run = client.post("http://localhost:8642/v1/runs")
        .json(&json!({
            "input": message,
            "session_id": session_id,
            "model": "deepseek-v4-pro"
        }))
        .send().await?;

    let run_id = run.json::<RunResponse>().await?.run_id;

    // 2. SSE 流式读取
    let events = client.get(format!("http://localhost:8642/v1/runs/{}/events", run_id))
        .send().await?;

    // 3. 逐事件处理 → 发射到前端
    while let Some(event) = events.next().await {
        emit_to_frontend("hermes:event", event);
    }
}
```

### 4.3 会话管理

VoiceDesk 需要独立管理会话 ID，以保持对话上下文：

```rust
struct SessionManager {
    current_session_id: Option<String>,
    history: Vec<ConversationTurn>,
    // 存储在本地 SQLite
    db: SqliteConnection,
}

struct ConversationTurn {
    id: String,
    user_text: String,
    ai_text: String,
    timestamp: DateTime<Utc>,
    session_id: String,
}
```

### 4.4 可选：与 shujietai 集成

```typescript
// 将语音对话中的重要内容保存为 shujietai 任务
async function createFollowUpTask(title: string, context: string) {
  await fetch('http://localhost:18000/api/v1/task-board', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      title,
      lane: 'todo',
      priority: 3,
      description: `来自语音对话: ${context}`
    })
  });
}
```

---

## 5. 项目结构

```
hermes-voicedesk/
├── src-tauri/                # Rust 后端
│   ├── Cargo.toml
│   ├── tauri.conf.json       # Tauri 配置（窗口、托盘、权限）
│   ├── capabilities/         # Tauri v2 权限声明
│   ├── src/
│   │   ├── main.rs           # 入口，Tauri 初始化
│   │   ├── lib.rs            # 核心模块注册
│   │   ├── audio/
│   │   │   ├── mod.rs
│   │   │   ├── capture.rs    # 音频采集（cpal + coreaudio）
│   │   │   ├── vad.rs        # VAD 引擎（silero-vad ONNX）
│   │   │   └── player.rs     # TTS 播放（AVSpeechSynthesizer）
│   │   ├── stt/
│   │   │   ├── mod.rs
│   │   │   ├── apple.rs      # NSSpeechRecognizer 封装
│   │   │   └── whisper.rs    # faster-whisper 调用
│   │   ├── api/
│   │   │   ├── mod.rs
│   │   │   └── hermes.rs     # Hermes API client（SSE 流式）
│   │   ├── session/
│   │   │   ├── mod.rs
│   │   │   └── store.rs      # SQLite 会话存储
│   │   ├── tray.rs           # 系统托盘
│   │   └── hotkey.rs         # 全局热键
│   └── icons/                # 应用图标
│
├── src/                      # Vue 3 前端
│   ├── App.vue
│   ├── main.ts
│   ├── views/
│   │   ├── VoiceChat.vue     # 主语音对话界面
│   │   └── Settings.vue      # 设置页
│   ├── components/
│   │   ├── AudioWave.vue     # 音频波形可视化
│   │   ├── Transcription.vue # 实时转写显示
│   │   ├── ResponseCard.vue  # AI 响应卡片（流式打字）
│   │   ├── StateIndicator.vue# 状态指示灯
│   │   └── TrayMenu.vue      # 托盘菜单
│   ├── stores/
│   │   ├── voiceSession.ts   # 语音会话状态（Pinia）
│   │   └── settings.ts       # 用户设置
│   ├── composables/
│   │   ├── useHermes.ts      # Hermes API 通信
│   │   ├── useAudio.ts       # 音频 Tauri 事件监听
│   │   └── useHotkey.ts      # 热键管理
│   └── styles/
│       └── voice.css         # 语音界面样式
│
├── package.json
├── vite.config.ts
├── tsconfig.json
└── README.md
```

---

## 6. 关键技术细节

### 6.1 音频采集参数

```
采样率: 16 kHz (whisper 最佳)
位深: 16-bit
声道: Mono
缓冲帧大小: 480 samples (30ms @ 16kHz) — 匹配 silero-vad 要求
```

### 6.2 环形缓冲区

```rust
use std::collections::VecDeque;

struct RingBuffer {
    buffer: VecDeque<i16>,
    max_frames: usize,    // 30 秒 = 480,000 samples
    speech_start_idx: Option<usize>,
}

impl RingBuffer {
    fn push(&mut self, samples: &[i16]) {
        self.buffer.extend(samples);
        // 保持最大长度
        while self.buffer.len() > self.max_frames {
            self.buffer.pop_front();
        }
    }

    fn get_speech_segment(&self) -> &[i16] {
        // 返回从 speech_start_idx 到当前的片段
        // 包含 500ms 的前导静音（VAD 回溯）
    }
}
```

### 6.3 VAD 参数调优

```rust
const VAD_THRESHOLD: f32 = 0.5;          // 语音概率阈值
const SPEECH_START_FRAMES: u32 = 3;       // 连续 3 帧(90ms)确认开始
const SPEECH_END_FRAMES: u32 = 20;        // 连续 20 帧(600ms)确认结束
const MAX_SPEECH_DURATION: f32 = 30.0;    // 最长单次录音 30 秒
const PRE_SPEECH_PADDING_MS: u32 = 500;   // 回溯 500ms 静音
```

### 6.4 前端-后端通信（Tauri Events）

```typescript
// 前端监听 Rust 端事件
import { listen } from '@tauri-apps/api/event';

// 音频状态
listen('audio:state', (event) => {
  // { state: 'listening' | 'thinking' | 'speaking' | 'idle' }
});

// 实时音量（用于波形可视化）
listen('audio:volume', (event) => {
  // { rms: 0.0-1.0 }
});

// STT 增量转写
listen('stt:partial', (event) => {
  // { text: '部分转写文本' }
});

// STT 最终结果
listen('stt:final', (event) => {
  // { text: '最终转写文本' }
});

// Hermes 响应流
listen('hermes:delta', (event) => {
  // { content: '响应文本增量' }
});

// 工具调用
listen('hermes:tool', (event) => {
  // { tool: 'web_search', status: 'started' | 'completed' }
});
```

---

## 7. 实施计划（分 Phase）

### Phase 1: 最小可用原型 (MVP) — 2 周

**目标**：能说话、能听到回复

- [ ] Tauri v2 项目初始化 + Vue 3 frontend
- [ ] Rust 端音频采集 (cpal + coreaudio)
- [ ] VAD 检测 (silero-vad ONNX)
- [ ] macOS NSSpeechRecognizer 集成
- [ ] Hermes API 集成 (POST /v1/runs + SSE)
- [ ] macOS AVSpeechSynthesizer TTS
- [ ] 基础 UI：波形 + 转写文本 + AI 回复
- [ ] 系统托盘 + 全局热键唤醒

### Phase 2: 体验打磨 — 2 周

- [ ] 打断 (Barge-in) 机制
- [ ] 流式 TTS（收到第一个 token 就开始朗读）
- [ ] 对话历史 + SQLite 持久化
- [ ] 设置页（STT/TTS 引擎选择、VAD 灵敏度、热键自定义）
- [ ] 错误处理与重连

### Phase 3: shujietai 集成 — 1 周

- [ ] "创建任务" 指令（语音转 shujietai 任务）
- [ ] 查询任务状态
- [ ] 通知同步（shujietai 任务完成 → VoiceDesk 通知）

### Phase 4: 进阶功能 — 2 周

- [ ] 多语言支持
- [ ] 自定义 TTS 音色
- [ ] 离线模式 (faster-whisper 本地 STT)
- [ ] 对话摘要与导出
- [ ] 跨平台适配 (Windows/Linux)

---

## 8. 风险与缓解

| 风险 | 影响 | 缓解方案 |
|------|------|----------|
| Tauri Rust 音频开发复杂度高 | 延期 | Phase 1 先用 Web Audio API + Web Speech API 做前端原型，Rust 端逐步替换 |
| macOS 权限弹窗体验差 | 用户流失 | 首次启动引导页说明，Tauri 有 `microphone` 权限插件 |
| silero-vad ONNX 在 Rust 集成困难 | VAD 不可用 | 备选：用 wasm-bindgen 在前端运行，或用 WebRTC VAD |
| Hermes API 不可用 | 完全无法工作 | 检测 + 引导用户启动 API server，显示连接状态指示灯 |
| 长时间运行内存泄漏 | 体验下降 | 添加内存监控，定期清理对话历史 |

---

## 9. 参考项目

| 项目 | 亮点 | 可借鉴 |
|------|------|--------|
| [LiveKit Agents](https://github.com/livekit/agents) | 实时语音 AI Agent 框架 | WebRTC 音频流、多模态 pipeline、打断机制 |
| [OpenAI Realtime API](https://platform.openai.com/docs/guides/realtime) | WebSocket 实时语音 API | 音频格式（PCM16 24kHz）、VAD 事件、函数调用 |
| [Ultravox](https://github.com/fixie-ai/ultravox) | 快速多模态 LLM 语音模型 | 端到端语音理解、低延迟架构 |
| [OpenAI Realtime Console](https://github.com/openai/openai-realtime-console) | WebRTC 实时语音前端参考 | 前端音频处理、波形可视化、工具调用展示 |
| [Silero VAD](https://github.com/snakers4/silero-vad) | 企业级 VAD 模型 | ONNX 推理、多语言支持、<1ms 延迟 |
| [faster-whisper](https://github.com/SYSTRAN/faster-whisper) | CTranslate2 加速 Whisper | M4 Max 上极速推理、低内存占用 |
| [Piper TTS](https://github.com/rhasspy/piper) | 本地神经 TTS | Rust 原生、低延迟、离线运行 |
| [Apple Speech Framework](https://developer.apple.com/documentation/speech) | macOS 系统级语音识别 | Neural Engine 加速、零延迟、免费 |

## 10. 架构对比：OpenAI Realtime API 模式 vs Hermes 当前模式

### OpenAI Realtime API 模式（参考）

```
WebSocket (wss://)
  ├── 音频上行: PCM16 24kHz mono (实时流)
  ├── VAD 事件: speech_started / speech_stopped (服务端检测)
  ├── 文本下行: response.text.delta (流式)
  ├── 音频下行: response.audio.delta (流式 TTS)
  └── 函数调用: response.function_call_arguments.done
```

优点：单 WebSocket 连接，延迟极低，无需额外 VAD/STT/TTS
缺点：必须用 OpenAI 的服务，不可替换后端

### Hermes 当前模式（适配方案）

```
HTTP POST /v1/runs → SSE GET /v1/runs/{id}/events
  ├── 客户端 VAD + STT → 文本
  ├── POST 文本 → Hermes run
  ├── SSE 接收: message.delta / tool.started / tool.completed
  └── 客户端 TTS → 音频播放
```

优点：与现有 Hermes API 完全兼容，可复用 shujietai 的 connector 代码
缺点：多段流水线，端到端延迟略高

### 混合模式（推荐）

VoiceDesk 可以采用"类 Realtime API"的架构封装 Hermes：

```
WebSocket (本地) ↔ VoiceDesk Rust Backend
  ├── 前端→后端: 音频 chunk (PCM)
  ├── 后端→VAD→STT→Hermes API
  ├── 后端→前端: 文本 delta + 音频 chunk
  └── 后端→前端: 状态变化 + 工具调用
```

这样前端只需要处理一个 WebSocket 连接，复杂逻辑全部在 Rust 后端。

## 11. Tauri v2 关键插件清单

| 插件 | 用途 | Cargo.toml |
|------|------|------------|
| `tauri-plugin-shell` | 命令行调用（启动 Hermes API） | 内置 |
| `tauri-plugin-global-shortcut` | 全局热键 | `tauri-plugin-global-shortcut` |
| `tauri-plugin-notification` | 系统通知 | `tauri-plugin-notification` |
| `tauri-plugin-dialog` | 文件选择/权限对话框 | `tauri-plugin-dialog` |
| `tauri-plugin-process` | 进程管理 | `tauri-plugin-process` |
| `tauri-plugin-fs` | 文件系统访问（日志/缓存） | `tauri-plugin-fs` |
| `tauri-plugin-sql` | SQLite 会话存储 | `tauri-plugin-sql` |

## 12. 附录：为何不选 WebRTC

对于桌面应用场景，WebRTC 带来不必要的复杂度：

1. **信令服务器**：WebRTC 需要 STUN/TURN 服务器和信令通道，而桌面 app 直接 localhost 通信
2. **NAT 穿透**：本地通信不需要
3. **编解码开销**：Opus 编解码增加延迟，本地可用原始 PCM
4. **复杂性**：WebRTC 实现通常需要 5000+ 行代码

**何时考虑 WebRTC**：如果未来需要远程访问（手机连接桌面 AI），可引入 LiveKit 作为 WebRTC 中继

---

## 13. 总结

**推荐方案**：Tauri v2 + Vue 3 + Rust，独立 GitHub 仓库 `hermes-voicedesk`。

**核心优势**：
- 包体积小（~10MB），适合常驻托盘
- 与现有 Vue 3 技能栈匹配，前端代码可复用
- Rust 端保证音频处理低延迟
- 直接对接 Hermes API，架构简洁
- 可选集成 shujietai，松耦合设计

**关键创新点**：
- 真正的打断（Barge-in）支持，而非简单的静音检测
- 流式 TTS（边生成边说），减少感知延迟
- 系统级集成（托盘、热键、通知），而非网页套壳
