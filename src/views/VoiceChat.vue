<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import AudioWave from '../components/AudioWave.vue'
import Transcription from '../components/Transcription.vue'
import ChatBubble from '../components/ChatBubble.vue'
import StateIndicator from '../components/StateIndicator.vue'
import DebugPanel from '../components/DebugPanel.vue'
import type { DebugEntry } from '../components/DebugPanel.vue'

type VoiceState = 'idle' | 'waiting' | 'listening' | 'transcribing' | 'thinking' | 'responding' | 'speaking'

interface Message {
  role: 'user' | 'ai'
  text: string
}

const state = ref<VoiceState>('waiting')
const userText = ref('')
const messages = ref<Message[]>([])
const apiConnected = ref(false)
const isListening = ref(false)
const volume = ref(0)
const transcribedText = ref('')
const wakeMode = ref<'vad' | 'porcupine' | ''>('')
const wakeKeyword = ref('')
const wakeEnabled = ref(true)

// Session ID — date-based for grouping turns by day
const sessionId = ref(`voice-${new Date().toISOString().slice(0, 10)}`)

// Chat area ref for scroll-to-bottom
const chatArea = ref<HTMLElement | null>(null)

// Debug log panel
const debugVisible = ref(false)
const debugLog = ref<DebugEntry[]>([])

function addDebugEntry(
  type: DebugEntry['type'],
  category: string,
  message: string,
  detail?: string
) {
  const now = new Date()
  const timestamp = now.toLocaleTimeString('en-US', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }) + '.' + String(now.getMilliseconds()).padStart(3, '0')
  debugLog.value.push({ timestamp, type, category, message, detail })
}

function clearDebugLog() {
  debugLog.value = []
}

// --- Sentence-boundary streaming TTS ---
const sentenceBuffer = ref('')
const ttsQueue: string[] = []
let isTtsActive = false
let isProcessing = false
let responseFinished = false

// Regex for sentence-ending boundaries
const SENTENCE_END_RE = /([。！？]|\n\n|[.!?](?=\s|$))/

function extractSentences(text: string): { sentences: string[]; remainder: string } {
  const sentences: string[] = []
  let working = text
  let lastEnd = 0
  const re = new RegExp(SENTENCE_END_RE.source, 'g')
  let match: RegExpExecArray | null

  while ((match = re.exec(working)) !== null) {
    const endIdx = match.index + match[0].length
    const sentence = working.substring(lastEnd, endIdx).trim()
    if (sentence) {
      sentences.push(sentence)
    }
    lastEnd = endIdx
    re.lastIndex = lastEnd
  }

  const remainder = working.substring(lastEnd)
  return { sentences, remainder }
}

function enqueueSentence(sentence: string) {
  const trimmed = sentence.trim()
  if (!trimmed) return
  ttsQueue.push(trimmed)
  if (!isTtsActive) {
    speakNextInQueue()
  }
}

function speakNextInQueue() {
  if (ttsQueue.length === 0) {
    isTtsActive = false
    if (responseFinished) {
      // Go back to wake word mode instead of idle
      enterWakeMode()
    }
    return
  }

  isTtsActive = true
  state.value = 'speaking'
  const sentence = ttsQueue.shift()!
  invoke('speak_text', { text: sentence }).catch((e) => {
    console.error('speak_text failed:', e)
    speakNextInQueue()
  })
}

function scrollToBottom() {
  nextTick(() => {
    if (chatArea.value) {
      chatArea.value.scrollTop = chatArea.value.scrollHeight
    }
  })
}

function enterWakeMode() {
  console.log(`[VoiceChat] enterWakeMode (wakeEnabled=${wakeEnabled.value})`)
  addDebugEntry('info', 'STATE', '→ waiting (wake)', `wakeEnabled=${wakeEnabled.value}`)
  if (!wakeEnabled.value) {
    state.value = 'idle'
    return
  }
  // Stop any active mic capture
  invoke('stop_listening').catch(() => {})
  isListening.value = false

  // Start wake word detection
  invoke('start_wake_word', {
    accessKey: null,
    keyword: 'picovoice',
  }).catch((e) => console.error('start_wake_word failed:', e))

  state.value = 'waiting'
}

let unlisteners: Array<() => void> = []

onMounted(async () => {
  // Load chat history from disk
  try {
    const turns = await invoke<Array<{ user_text: string; ai_text: string }>>('load_chat_history', {
      sessionId: sessionId.value,
    })
    for (const turn of turns) {
      messages.value.push({ role: 'user', text: turn.user_text })
      messages.value.push({ role: 'ai', text: turn.ai_text })
    }
    if (turns.length > 0) {
      scrollToBottom()
    }
  } catch (e) {
    console.error('Failed to load chat history:', e)
  }

  try {
    apiConnected.value = await invoke('check_hermes_api')
    addDebugEntry('success', 'SYSTEM', 'Hermes API connected')
  } catch (e) {
    apiConnected.value = false
    console.error('Hermes API check failed:', e)
    addDebugEntry('error', 'SYSTEM', 'Hermes API unreachable', String(e))
  }

  const u1 = await listen<{ state: VoiceState }>('audio:state', (event) => {
    if (!isTtsActive && !isProcessing) {
      console.log(`[VoiceChat] audio:state → ${event.payload.state} (applied)`)
      state.value = event.payload.state
      addDebugEntry('info', 'STATE', `→ ${event.payload.state}`)
    } else {
      console.log(`[VoiceChat] audio:state → ${event.payload.state} (ignored: isTtsActive=${isTtsActive}, isProcessing=${isProcessing})`)
      addDebugEntry('warning', 'STATE', `${event.payload.state} (ignored)`, `TTS active or processing`)
    }
  })

  const u2 = await listen<{ rms: number; pct: number }>('audio:volume', (event) => {
    volume.value = (event.payload.pct || event.payload.rms * 1000) / 100
  })

  // Wake word events
  const u9 = await listen<{ state: string; mode: string; keyword: string }>(
    'wake:state',
    (event) => {
      wakeMode.value = event.payload.mode as 'vad' | 'porcupine'
      wakeKeyword.value = event.payload.keyword
      state.value = 'waiting'
      addDebugEntry('info', 'WAKE', `Wake mode: ${event.payload.mode}`, `keyword="${event.payload.keyword}"`)
    }
  )

  const u10 = await listen<{ keyword: string; mode?: string }>(
    'wake:detected',
    async () => {
      addDebugEntry('success', 'WAKE', 'Wake word detected!', 'Starting listening...')
      // Wake word detected! Stop wake word and start listening.
      invoke('stop_wake_word').catch(() => {})
      // Small delay for clean transition
      await new Promise((r) => setTimeout(r, 200))
      await startListening()
    }
  )

  const u11 = await listen<{ error: string }>('wake:error', (event) => {
    console.error('Wake word error:', event.payload.error)
    addDebugEntry('error', 'WAKE', 'Wake error', event.payload.error)
    // Fall back to idle so user can manually listen
    state.value = 'idle'
  })

  // Voice pipeline: audio captured → transcribed → Hermes
  const u3 = await listen<{ text: string }>('stt:result', async (event) => {
    const text = event.payload.text
    console.log(`[VoiceChat] stt:result text="${text}"`)
    addDebugEntry('success', 'STT', `Transcribed`, `text="${text}"`)
    if (!text || text.startsWith('[')) {
      console.log(`[VoiceChat] stt:result → empty/failed, showing error`)
      addDebugEntry('warning', 'STT', 'Empty or failed transcription', `raw="${text}"`)
      messages.value.push({ role: 'ai', text: "Sorry, I didn't catch that." })
      scrollToBottom()
      state.value = 'idle'
      return
    }

    // Stop any in-progress TTS
    invoke('stop_speaking').catch(() => {})
    ttsQueue.length = 0
    isTtsActive = false
    sentenceBuffer.value = ''
    responseFinished = false

    // Add user message to chat history
    messages.value.push({ role: 'user', text })
    // Add placeholder for AI response
    messages.value.push({ role: 'ai', text: '' })
    scrollToBottom()

    console.log(`[VoiceChat] → hermes_chat_stream: "${text}"`)
    isProcessing = true
    state.value = 'thinking'
    addDebugEntry('info', 'API', '→ hermes_chat_stream', `message="${text}"`)

    try {
      await invoke('hermes_chat_stream', { message: text })
      isProcessing = false
    } catch (e) {
      isProcessing = false
      state.value = 'idle'
      console.error(`[VoiceChat] hermes_chat_stream error:`, e)
      addDebugEntry('error', 'API', 'hermes_chat_stream failed', String(e))
      const lastMsg = messages.value[messages.value.length - 1]
      if (lastMsg && lastMsg.role === 'ai') {
        lastMsg.text = `Error: ${e}`
      }
    }
  })

  // Streaming delta — accumulate into the last AI message
  const u4 = await listen<{ content: string }>('hermes:delta', (event) => {
    state.value = 'responding'

    // Update the last AI message in the array
    if (messages.value.length > 0 && messages.value[messages.value.length - 1].role === 'ai') {
      messages.value[messages.value.length - 1].text += event.payload.content
    }

    addDebugEntry('info', 'API', `Delta: +${event.payload.content.length} chars`, event.payload.content.length > 100 ? event.payload.content.slice(0, 100) + '...' : event.payload.content)

    sentenceBuffer.value += event.payload.content
    const { sentences, remainder } = extractSentences(sentenceBuffer.value)
    for (const s of sentences) {
      enqueueSentence(s)
    }
    sentenceBuffer.value = remainder
    scrollToBottom()
  })

  const u5 = await listen<{ tool: string; status: string }>('hermes:tool', (event) => {
    addDebugEntry('info', 'API', `Tool: ${event.payload.tool}`, `status=${event.payload.status}`)
  })

  const u6 = await listen('hermes:finish', () => {
    responseFinished = true
    addDebugEntry('success', 'API', 'Stream finished', `Total AI message length: ${messages.value[messages.value.length - 1]?.text.length || 0}`)

    // Flush any remaining text in the buffer
    if (sentenceBuffer.value.trim()) {
      enqueueSentence(sentenceBuffer.value)
      sentenceBuffer.value = ''
    }

    // Save this turn to history
    const msgs = messages.value
    if (msgs.length >= 2) {
      const lastAi = msgs[msgs.length - 1]
      const lastUser = msgs[msgs.length - 2]
      if (lastAi.role === 'ai' && lastUser.role === 'user') {
        invoke('save_chat_history', {
          sessionId: sessionId.value,
          userText: lastUser.text,
          aiText: lastAi.text,
        }).catch((e) => console.error('Failed to save chat history:', e))
      }
    }

    if (ttsQueue.length === 0 && !isTtsActive) {
      // Go back to wake word mode
      enterWakeMode()
    }
  })

  const u7 = await listen<{ error: string }>('hermes:error', (event) => {
    addDebugEntry('error', 'API', 'Hermes error', event.payload.error)
    const lastMsg = messages.value[messages.value.length - 1]
    if (lastMsg && lastMsg.role === 'ai') {
      lastMsg.text += `\n\nError: ${event.payload.error}`
    }
    responseFinished = true
    enterWakeMode()
  })

  // TTS completion
  const u8 = await listen('tts:complete', () => {
    addDebugEntry('info', 'TTS', 'TTS sentence complete')
    speakNextInQueue()
  })

  unlisteners = [u1, u2, u3, u4, u5, u6, u7, u8, u9, u10, u11]

  // Auto-start wake word detection on app launch
  enterWakeMode()
})

onUnmounted(() => {
  unlisteners.forEach((u) => u())
  invoke('stop_wake_word').catch(() => {})
})

async function startListening() {
  console.log('[VoiceChat] startListening')
  addDebugEntry('info', 'STATE', '→ listening', 'Mic started')
  sentenceBuffer.value = ''
  ttsQueue.length = 0
  isTtsActive = false
  isProcessing = false
  responseFinished = false
  await invoke('start_listening')
  isListening.value = true
}

async function toggleListening() {
  if (isListening.value) {
    await invoke('stop_listening')
    isListening.value = false
    enterWakeMode()
  } else {
    // Stop wake word first
    await invoke('stop_wake_word')
    await startListening()
  }
}

async function sendText() {
  if (!userText.value.trim()) return

  const text = userText.value.trim()
  userText.value = ''

  // Stop wake word
  await invoke('stop_wake_word')

  // Add user message to chat history
  messages.value.push({ role: 'user', text })
  // Add placeholder for AI response
  messages.value.push({ role: 'ai', text: '' })
  scrollToBottom()

  sentenceBuffer.value = ''
  ttsQueue.length = 0
  isTtsActive = false
  responseFinished = false
  isProcessing = true
  state.value = 'thinking'
  addDebugEntry('info', 'API', '→ hermes_chat_stream (text)', `message="${text}"`)

  try {
    await invoke('hermes_chat_stream', { message: text })
    isProcessing = false
  } catch (e) {
    isProcessing = false
    addDebugEntry('error', 'API', 'hermes_chat_stream failed (text)', String(e))
    const lastMsg = messages.value[messages.value.length - 1]
    if (lastMsg && lastMsg.role === 'ai') {
      lastMsg.text = `Error: ${e}`
    }
    enterWakeMode()
  }
}

async function toggleWake() {
  wakeEnabled.value = !wakeEnabled.value
  addDebugEntry('info', 'SYSTEM', `Wake word ${wakeEnabled.value ? 'enabled' : 'disabled'}`)
  if (!wakeEnabled.value) {
    await invoke('stop_wake_word')
    await invoke('stop_listening')
    isListening.value = false
    state.value = 'idle'
  } else {
    enterWakeMode()
  }
}
</script>

<template>
  <div class="voice-chat">
    <div class="main-content">
      <header class="chat-header">
      <StateIndicator :state="state" :api-connected="apiConnected" :wake-mode="wakeMode" :wake-keyword="wakeKeyword" />
      <div class="header-buttons">
        <button
          class="btn-debug"
          :class="{ active: debugVisible }"
          @click="debugVisible = !debugVisible"
          title="Toggle Debug Panel"
        >
          🐛
        </button>
        <button
          class="btn-wake"
          :class="{ active: wakeEnabled }"
          @click="toggleWake"
          :title="wakeEnabled ? 'Wake word ON' : 'Wake word OFF'"
        >
          {{ wakeEnabled ? '🎯' : '🔇' }}
        </button>
        <button class="btn-listen" :class="{ active: isListening }" @click="toggleListening">
          {{ isListening ? '⏹ Stop' : '🎤 Listen' }}
        </button>
      </div>
    </header>

    <AudioWave :active="state === 'listening'" :volume="volume" />

    <!-- Chat history — scrollable message bubbles area -->
    <div ref="chatArea" class="chat-area">
      <!-- Wake word waiting indicator -->
      <div v-if="state === 'waiting'" class="wake-waiting">
        <div class="wake-pulse"></div>
        <div class="wake-text">
          <span v-if="wakeMode === 'porcupine'">Listening for "{{ wakeKeyword }}"...</span>
          <span v-else>Speak to activate...</span>
        </div>
        <div class="wake-hint">Say something to start</div>
      </div>

      <ChatBubble
        v-for="(msg, i) in messages"
        :key="i"
        :role="msg.role"
        :text="msg.text"
      />

      <div v-if="state === 'thinking' || state === 'transcribing'" class="thinking-indicator">
        <span class="dot"></span>
        <span class="dot"></span>
        <span class="dot"></span>
        <span class="thinking-text">{{ state === 'transcribing' ? 'Transcribing...' : 'Thinking...' }}</span>
      </div>
    </div>

    <Transcription
      :text="userText"
      :placeholder="state === 'listening' ? 'Listening...' : state === 'waiting' ? 'Say wake word or type...' : state === 'transcribing' ? 'Transcribing speech...' : state === 'thinking' ? transcribedText || 'Processing...' : 'Type or speak...'"
      @update:text="userText = $event"
      @send="sendText"
    />

    <footer class="status-bar">
      <span class="status-dot" :class="{ connected: apiConnected }"></span>
      Hermes API {{ apiConnected ? 'Connected' : 'Disconnected' }}
      <span v-if="wakeEnabled" class="wake-indicator">
        | Wake: {{ wakeMode === 'porcupine' ? `"${wakeKeyword}"` : 'VAD' }}
      </span>
      <span v-if="isListening" class="mic-level">| Mic: {{ (volume * 100).toFixed(0) }}%</span>
    </footer>
    </div>

    <DebugPanel
      :entries="debugLog"
      :visible="debugVisible"
      @close="debugVisible = false"
      @clear="clearDebugLog"
    />
  </div>
</template>

<style scoped>
.voice-chat {
  display: flex;
  flex-direction: row;
  height: 100vh;
  background: #1a1a2e;
  color: #e0e0e0;
  font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Display', sans-serif;
  overflow: hidden;
}

.main-content {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.chat-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 16px;
  border-bottom: 1px solid #2a2a4a;
  background: #16162a;
  flex-shrink: 0;
}

.header-buttons {
  display: flex;
  gap: 8px;
  align-items: center;
}

.btn-listen {
  padding: 8px 20px;
  border-radius: 20px;
  border: 1px solid #4a4a8a;
  background: transparent;
  color: #a0a0d0;
  cursor: pointer;
  font-size: 14px;
  transition: all 0.2s;
}

.btn-listen.active {
  background: #e74c3c;
  border-color: #e74c3c;
  color: white;
}

.btn-listen:hover {
  opacity: 0.9;
}

.btn-wake {
  padding: 6px 10px;
  border-radius: 20px;
  border: 1px solid #4a4a8a;
  background: transparent;
  cursor: pointer;
  font-size: 16px;
  transition: all 0.2s;
  line-height: 1;
}

.btn-wake.active {
  border-color: #6c5ce7;
  background: rgba(108, 92, 231, 0.15);
}

.btn-wake:hover {
  opacity: 0.9;
}

.btn-debug {
  padding: 6px 10px;
  border-radius: 20px;
  border: 1px solid #4a4a8a;
  background: transparent;
  cursor: pointer;
  font-size: 14px;
  transition: all 0.2s;
  line-height: 1;
}

.btn-debug.active {
  border-color: #e5c07b;
  background: rgba(229, 192, 123, 0.15);
}

.btn-debug:hover {
  opacity: 0.9;
}

/* Wake word waiting indicator */
.wake-waiting {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 48px 16px;
  gap: 16px;
  opacity: 0.8;
}

.wake-pulse {
  width: 64px;
  height: 64px;
  border-radius: 50%;
  background: rgba(108, 92, 231, 0.15);
  border: 2px solid rgba(108, 92, 231, 0.4);
  animation: wakePulse 2s ease-in-out infinite;
}

.wake-text {
  font-size: 16px;
  color: #a0a0d0;
  text-align: center;
}

.wake-hint {
  font-size: 12px;
  color: #666;
}

@keyframes wakePulse {
  0%, 100% {
    transform: scale(0.9);
    opacity: 0.6;
  }
  50% {
    transform: scale(1.05);
    opacity: 1;
  }
}

/* Chat area — scrollable message list */
.chat-area {
  flex: 1;
  overflow-y: auto;
  padding: 12px 0;
}

.chat-area::-webkit-scrollbar {
  width: 6px;
}

.chat-area::-webkit-scrollbar-track {
  background: transparent;
}

.chat-area::-webkit-scrollbar-thumb {
  background: #3a3a6a;
  border-radius: 3px;
}

.thinking-indicator {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 12px 16px;
}

.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #6c5ce7;
  animation: bounce 1.2s ease-in-out infinite;
}

.dot:nth-child(2) { animation-delay: 0.2s; }
.dot:nth-child(3) { animation-delay: 0.4s; }

.thinking-text {
  margin-left: 8px;
  color: #888;
  font-size: 14px;
}

.status-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  font-size: 12px;
  color: #888;
  border-top: 1px solid #2a2a4a;
  background: #16162a;
  flex-shrink: 0;
}

.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #666;
}

.status-dot.connected {
  background: #2ecc71;
}

.mic-level {
  color: #6c5ce7;
}

.wake-indicator {
  color: #6c5ce7;
}

@keyframes bounce {
  0%, 100% { transform: translateY(0); }
  50% { transform: translateY(-8px); }
}
</style>
