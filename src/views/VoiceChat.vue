<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import AudioWave from '../components/AudioWave.vue'
import Transcription from '../components/Transcription.vue'
import ChatBubble from '../components/ChatBubble.vue'
import StateIndicator from '../components/StateIndicator.vue'

type VoiceState = 'idle' | 'listening' | 'thinking' | 'responding' | 'speaking'

interface Message {
  role: 'user' | 'ai'
  text: string
}

const state = ref<VoiceState>('idle')
const userText = ref('')
const messages = ref<Message[]>([])
const apiConnected = ref(false)
const isListening = ref(false)
const volume = ref(0)
const transcribedText = ref('')

// Session ID — date-based for grouping turns by day
const sessionId = ref(`voice-${new Date().toISOString().slice(0, 10)}`)

// Chat area ref for scroll-to-bottom
const chatArea = ref<HTMLElement | null>(null)

// --- Sentence-boundary streaming TTS ---
const sentenceBuffer = ref('')
const ttsQueue: string[] = []
let isTtsActive = false
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
      state.value = 'idle'
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
  } catch (e) {
    apiConnected.value = false
    console.error('Hermes API check failed:', e)
  }

  const u1 = await listen<{ state: VoiceState }>('audio:state', (event) => {
    if (!isTtsActive) {
      state.value = event.payload.state
    }
  })

  const u2 = await listen<{ rms: number; pct: number }>('audio:volume', (event) => {
    volume.value = (event.payload.pct || event.payload.rms * 1000) / 100
  })

  // Voice pipeline: audio captured → transcribed → Hermes
  const u3 = await listen<{ text: string }>('stt:result', async (event) => {
    const text = event.payload.text
    if (!text || text.startsWith('[')) {
      state.value = 'listening'
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

    state.value = 'thinking'

    try {
      await invoke('hermes_chat_stream', { message: text })
    } catch (e) {
      state.value = 'idle'
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

    sentenceBuffer.value += event.payload.content
    const { sentences, remainder } = extractSentences(sentenceBuffer.value)
    for (const s of sentences) {
      enqueueSentence(s)
    }
    sentenceBuffer.value = remainder
    scrollToBottom()
  })

  const u5 = await listen<{ tool: string; status: string }>('hermes:tool', (_event) => {
    // Tool calls are shown inline via delta stream; no separate display needed
  })

  const u6 = await listen('hermes:finish', () => {
    responseFinished = true

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
      state.value = 'idle'
    }
  })

  const u7 = await listen<{ error: string }>('hermes:error', (event) => {
    state.value = 'idle'
    const lastMsg = messages.value[messages.value.length - 1]
    if (lastMsg && lastMsg.role === 'ai') {
      lastMsg.text += `\n\nError: ${event.payload.error}`
    }
    responseFinished = true
  })

  // TTS completion
  const u8 = await listen('tts:complete', () => {
    speakNextInQueue()
  })

  unlisteners = [u1, u2, u3, u4, u5, u6, u7, u8]
})

onUnmounted(() => {
  unlisteners.forEach((u) => u())
})

async function toggleListening() {
  if (isListening.value) {
    await invoke('stop_listening')
    isListening.value = false
    state.value = 'idle'
  } else {
    sentenceBuffer.value = ''
    ttsQueue.length = 0
    isTtsActive = false
    responseFinished = false
    await invoke('start_listening')
    isListening.value = true
  }
}

async function sendText() {
  if (!userText.value.trim()) return

  const text = userText.value.trim()
  userText.value = ''

  // Add user message to chat history
  messages.value.push({ role: 'user', text })
  // Add placeholder for AI response
  messages.value.push({ role: 'ai', text: '' })
  scrollToBottom()

  sentenceBuffer.value = ''
  ttsQueue.length = 0
  isTtsActive = false
  responseFinished = false
  state.value = 'thinking'

  try {
    await invoke('hermes_chat_stream', { message: text })
  } catch (e) {
    state.value = 'idle'
    const lastMsg = messages.value[messages.value.length - 1]
    if (lastMsg && lastMsg.role === 'ai') {
      lastMsg.text = `Error: ${e}`
    }
  }
}
</script>

<template>
  <div class="voice-chat">
    <header class="chat-header">
      <StateIndicator :state="state" :api-connected="apiConnected" />
      <button class="btn-listen" :class="{ active: isListening }" @click="toggleListening">
        {{ isListening ? '⏹ Stop' : '🎤 Listen' }}
      </button>
    </header>

    <AudioWave :active="state === 'listening'" :volume="volume" />

    <!-- Chat history — scrollable message bubbles area -->
    <div ref="chatArea" class="chat-area">
      <ChatBubble
        v-for="(msg, i) in messages"
        :key="i"
        :role="msg.role"
        :text="msg.text"
      />

      <div v-if="state === 'thinking'" class="thinking-indicator">
        <span class="dot"></span>
        <span class="dot"></span>
        <span class="dot"></span>
        <span class="thinking-text">Thinking...</span>
      </div>
    </div>

    <Transcription
      :text="userText"
      :placeholder="state === 'listening' ? 'Listening...' : state === 'thinking' ? transcribedText || 'Processing...' : 'Type or speak...'"
      @update:text="userText = $event"
      @send="sendText"
    />

    <footer class="status-bar">
      <span class="status-dot" :class="{ connected: apiConnected }"></span>
      Hermes API {{ apiConnected ? 'Connected' : 'Disconnected' }}
      <span v-if="isListening" class="mic-level">| Mic: {{ (volume * 100).toFixed(0) }}%</span>
    </footer>
  </div>
</template>

<style scoped>
.voice-chat {
  display: flex;
  flex-direction: column;
  height: 100vh;
  background: #1a1a2e;
  color: #e0e0e0;
  font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Display', sans-serif;
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

@keyframes bounce {
  0%, 100% { transform: translateY(0); }
  50% { transform: translateY(-8px); }
}
</style>
