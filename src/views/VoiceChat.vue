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

let unlisteners: Array<() => void> = []

onMounted(async () => {
  // Check Hermes API health
  try {
    apiConnected.value = await invoke('check_hermes_api')
  } catch {
    apiConnected.value = false
  }

  // Listen to audio state changes
  const u1 = await listen<{ state: VoiceState }>('audio:state', (event) => {
    state.value = event.payload.state
  })

  // Listen to Hermes streaming deltas
  const u2 = await listen<{ content: string }>('hermes:delta', (event) => {
    state.value = 'responding'
    aiText.value += event.payload.content
  })

  // Listen to tool calls
  const u3 = await listen<{ tool: string; status: string; error?: boolean }>('hermes:tool', (event) => {
    toolCalls.value.push({
      tool: event.payload.tool,
      status: event.payload.status,
    })
  })

  // Listen to finish
  const u4 = await listen('hermes:finish', () => {
    state.value = 'speaking'
    // Trigger TTS
    invoke('speak_text', { text: aiText.value })
  })

  // Listen to errors
  const u5 = await listen<{ error: string }>('hermes:error', (event) => {
    state.value = 'idle'
    aiText.value += `\n\n❌ Error: ${event.payload.error}`
  })

  unlisteners = [u1, u2, u3, u4, u5]
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
    await invoke('start_listening')
    isListening.value = true
    state.value = 'listening'
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
    <!-- Header -->
    <header class="chat-header">
      <StateIndicator :state="state" :api-connected="apiConnected" />
      <button class="btn-listen" :class="{ active: isListening }" @click="toggleListening">
        {{ isListening ? '⏹ Stop' : '🎤 Listen' }}
      </button>
    </header>

    <!-- Audio Waveform -->
    <AudioWave :active="state === 'listening'" />

    <!-- Transcription -->
    <Transcription
      :text="userText"
      :placeholder="state === 'listening' ? 'Listening...' : 'Type or speak...'"
      @update:text="userText = $event"
      @send="sendText"
    />

    <!-- AI Response -->
    <ResponseCard
      :text="aiText"
      :tool-calls="toolCalls"
      :state="state"
    />

    <!-- Status bar -->
    <footer class="status-bar">
      <span class="status-dot" :class="{ connected: apiConnected }"></span>
      Hermes API {{ apiConnected ? 'Connected' : 'Disconnected' }}
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
</style>
