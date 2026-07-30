<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import AudioWave from '../components/AudioWave.vue'
import Transcription from '../components/Transcription.vue'
import ResponseCard from '../components/ResponseCard.vue'
import StateIndicator from '../components/StateIndicator.vue'

type VoiceState = 'idle' | 'listening' | 'thinking' | 'responding' | 'speaking'

const state = ref<VoiceState>('idle')
const userText = ref('')
const aiText = ref('')
const toolCalls = ref<Array<{ tool: string; status: string }>>([])
const apiConnected = ref(false)
const isListening = ref(false)
const volume = ref(0)
const transcribedText = ref('')

// --- Sentence-boundary streaming TTS ---
const sentenceBuffer = ref('')
const ttsQueue: string[] = []
let isTtsActive = false
let responseFinished = false

// Regex for sentence-ending boundaries:
//   。！？       — Chinese sentence enders
//   \n\n        — paragraph break
//   .!? followed by whitespace/end — English sentence enders
const SENTENCE_END_RE = /([。！？]|\n\n|[.!?](?=\s|$))/

/** Extract complete sentences from a text buffer. Returns the sentences found
 *  and the remaining partial text (no sentence boundary at the end). */
function extractSentences(text: string): { sentences: string[]; remainder: string } {
  const sentences: string[] = []
  let working = text
  let lastEnd = 0

  // Find all sentence boundaries using regex.exec in a loop
  const re = new RegExp(SENTENCE_END_RE.source, 'g')
  let match: RegExpExecArray | null

  while ((match = re.exec(working)) !== null) {
    const endIdx = match.index + match[0].length
    const sentence = working.substring(lastEnd, endIdx).trim()
    if (sentence) {
      sentences.push(sentence)
    }
    lastEnd = endIdx
    // Reset lastIndex for the substring-based approach
    re.lastIndex = lastEnd
  }

  const remainder = working.substring(lastEnd)
  return { sentences, remainder }
}

/** Enqueue a sentence for TTS. If nothing is currently speaking, starts the queue. */
function enqueueSentence(sentence: string) {
  const trimmed = sentence.trim()
  if (!trimmed) return
  ttsQueue.push(trimmed)
  if (!isTtsActive) {
    speakNextInQueue()
  }
}

/** Speak the next sentence in the queue. Called when TTS is idle and queue has items. */
function speakNextInQueue() {
  if (ttsQueue.length === 0) {
    isTtsActive = false
    // If the full response has finished and the queue is drained, go to idle
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
    // On error, skip to next sentence
    speakNextInQueue()
  })
}

let unlisteners: Array<() => void> = []

onMounted(async () => {
  try {
    apiConnected.value = await invoke('check_hermes_api')
  } catch (e) {
    apiConnected.value = false
    console.error('Hermes API check failed:', e)
  }

  const u1 = await listen<{ state: VoiceState }>('audio:state', (event) => {
    // Don't override state transitions driven by the sentence TTS queue
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
      // No speech detected or STT error
      state.value = 'listening'
      return
    }

    // User is speaking — immediately stop any in-progress TTS and clear queue
    invoke('stop_speaking').catch(() => {})
    ttsQueue.length = 0
    isTtsActive = false
    sentenceBuffer.value = ''
    responseFinished = false

    userText.value = text
    aiText.value = ''
    toolCalls.value = []
    state.value = 'thinking'

    try {
      await invoke('hermes_chat_stream', { message: text })
    } catch (e) {
      state.value = 'idle'
      aiText.value = `Error: ${e}`
    }
  })

  // Streaming delta — accumulate and split on sentence boundaries
  const u4 = await listen<{ content: string }>('hermes:delta', (event) => {
    state.value = 'responding'
    aiText.value += event.payload.content
    sentenceBuffer.value += event.payload.content

    // Check for complete sentences
    const { sentences, remainder } = extractSentences(sentenceBuffer.value)
    for (const s of sentences) {
      enqueueSentence(s)
    }
    sentenceBuffer.value = remainder
  })

  const u5 = await listen<{ tool: string; status: string }>('hermes:tool', (event) => {
    toolCalls.value.push({
      tool: event.payload.tool,
      status: event.payload.status,
    })
  })

  const u6 = await listen('hermes:finish', () => {
    responseFinished = true

    // Flush any remaining text in the buffer as a final sentence
    if (sentenceBuffer.value.trim()) {
      enqueueSentence(sentenceBuffer.value)
      sentenceBuffer.value = ''
    }

    // If nothing is queued and nothing is speaking, go to idle
    if (ttsQueue.length === 0 && !isTtsActive) {
      state.value = 'idle'
    }
    // Otherwise the queue processor (tts:complete listener) will handle the transition
  })

  const u7 = await listen<{ error: string }>('hermes:error', (event) => {
    state.value = 'idle'
    aiText.value += `\n\nError: ${event.payload.error}`
    responseFinished = true
  })

  // TTS completion — fires after each sentence finishes speaking via macOS `say`
  const u8 = await listen('tts:complete', () => {
    // Speak the next sentence in the queue
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
    aiText.value = ''
    toolCalls.value = []
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

  const text = userText.value
  userText.value = ''
  aiText.value = ''
  toolCalls.value = []
  sentenceBuffer.value = ''
  ttsQueue.length = 0
  isTtsActive = false
  responseFinished = false
  state.value = 'thinking'

  try {
    await invoke('hermes_chat_stream', { message: text })
  } catch (e) {
    state.value = 'idle'
    aiText.value = `Error: ${e}`
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

    <Transcription
      :text="userText"
      :placeholder="state === 'listening' ? 'Listening...' : state === 'thinking' ? transcribedText || 'Processing...' : 'Type or speak...'"
      @update:text="userText = $event"
      @send="sendText"
    />

    <ResponseCard
      :text="aiText"
      :tool-calls="toolCalls"
      :state="state"
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

.status-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  font-size: 12px;
  color: #888;
  border-top: 1px solid #2a2a4a;
  background: #16162a;
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
</style>
