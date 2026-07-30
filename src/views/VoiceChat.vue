<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import AudioWave from './components/AudioWave.vue'
import Transcription from './components/Transcription.vue'
import ResponseCard from './components/ResponseCard.vue'
import StateIndicator from './components/StateIndicator.vue'

type VoiceState = 'idle' | 'listening' | 'thinking' | 'responding' | 'speaking'

const state = ref<VoiceState>('idle')
const userText = ref('')
const aiText = ref('')
const toolCalls = ref<Array<{ tool: string; status: string }>>([])
const apiConnected = ref(false)
const isListening = ref(false)
const volume = ref(0)
const transcribedText = ref('')

let unlisteners: Array<() => void> = []

onMounted(async () => {
  try {
    apiConnected.value = await invoke('check_hermes_api')
  } catch {
    apiConnected.value = false
  }

  const u1 = await listen<{ state: VoiceState }>('audio:state', (event) => {
    state.value = event.payload.state
  })

  const u2 = await listen<{ rms: number }>('audio:volume', (event) => {
    volume.value = event.payload.rms
  })

  // Voice pipeline: audio captured → transcribe → Hermes
  const u3 = await listen<{ path: string }>('stt:audio_file', async (event) => {
    state.value = 'thinking'
    transcribedText.value = 'Transcribing...'

    try {
      // Try faster-whisper first, fall back to empty
      let text = ''
      try {
        text = await invoke<string>('transcribe_audio', { path: event.payload.path })
      } catch {
        // Whisper not available, use placeholder
        text = '[Speech detected — STT engine not available]'
      }

      transcribedText.value = text
      if (text && text.trim()) {
        userText.value = text
      }

      // Send to Hermes
      aiText.value = ''
      toolCalls.value = []
      state.value = 'thinking'

      await invoke('hermes_chat_stream', { message: text || 'Hello' })
    } catch (e) {
      state.value = 'idle'
      aiText.value = `Error: ${e}`
    }
  })

  const u4 = await listen<{ content: string }>('hermes:delta', (event) => {
    state.value = 'responding'
    aiText.value += event.payload.content
  })

  const u5 = await listen<{ tool: string; status: string }>('hermes:tool', (event) => {
    toolCalls.value.push({
      tool: event.payload.tool,
      status: event.payload.status,
    })
  })

  const u6 = await listen('hermes:finish', () => {
    state.value = 'speaking'
    invoke('speak_text', { text: aiText.value })
    // After TTS, go back to listening if still in voice mode
    setTimeout(() => {
      if (isListening.value && state.value === 'speaking') {
        state.value = 'listening'
      }
    }, 1000)
  })

  const u7 = await listen<{ error: string }>('hermes:error', (event) => {
    state.value = 'idle'
    aiText.value += `\n\nError: ${event.payload.error}`
  })

  unlisteners = [u1, u2, u3, u4, u5, u6, u7]
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
