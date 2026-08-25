<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import AudioWave from '../components/AudioWave.vue'
import Transcription from '../components/Transcription.vue'
import ChatBubble from '../components/ChatBubble.vue'
import StateIndicator from '../components/StateIndicator.vue'
import DebugPanel from '../components/DebugPanel.vue'
import ToolCarousel from '../components/ToolCarousel.vue'
import type { ToolCall } from '../components/ToolCarousel.vue'
import type { DebugEntry } from '../components/DebugPanel.vue'

type VoiceState = 'idle' | 'waiting' | 'listening' | 'transcribing' | 'thinking' | 'responding' | 'speaking'

interface Message {
  id: number
  role: 'user' | 'ai'
  text: string
}

const state = ref<VoiceState>('waiting')
const userText = ref('')
const messages = ref<Message[]>([])
let nextMessageId = 1
const toolCalls = ref<ToolCall[]>([])
const apiConnected = ref(false)
const isListening = ref(false)
const volume = ref(0)
const transcribedText = ref('')
const wakeMode = ref<'vad' | 'porcupine' | ''>('')
const wakeKeyword = ref('')
const wakeEnabled = ref(true)
const bargeInEnabled = ref(true)

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

// --- Staged TTS: flush sentence groups while the response is still streaming ---
const STAGED_TTS_SEGMENT_SIZE = 3
const STAGED_TTS_SILENCE_MS = 1500
const sentenceBuffer = ref('')
const pendingSentences: string[] = []
const queuedSegmentText = new Map<number, string>()
let nextQueuedSegmentIndex = 0
let isTtsActive = false
let isProcessing = false
let responseFinished = false
let lastDeltaAt = 0
let stagedFlushTimer: ReturnType<typeof setTimeout> | null = null
// Timestamp of the last completed TTS playback. STT results arriving within
// a short window after it are late stragglers (in-flight transcription of
// echo / residual buffer) and must NOT start a new conversation turn.
let lastTtsCompleteAt = 0

function resetStagedTtsState() {
  queuedSegmentText.clear()
  nextQueuedSegmentIndex = 0
}

// Hidden buffer that accumulates ALL deltas (full response text for history save)
const fullResponseText = ref('')

// Known abbreviations that should NOT end a sentence when followed by a period
// Includes: titles, months, common Latin abbreviations, and single-letter initials
const ABBREVIATIONS = new Set([
  'mr', 'mrs', 'ms', 'dr', 'prof', 'sr', 'jr', 'st',
  'rev', 'hon', 'capt', 'lt', 'col', 'gen', 'sgt', 'maj',
  'rep', 'sen', 'gov', 'pres', 'vp',
  'jan', 'feb', 'mar', 'apr', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec',
  'vs', 'etc', 'inc', 'ltd', 'co', 'corp', 'dept', 'est', 'approx',
  'vol', 'ed', 'no',
  // Single-letter initials (A., B., etc.) common in names like "John F. Kennedy"
  'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm',
  'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
])

// Matches all potential sentence-ending positions: Chinese punctuation, newlines, and .!?
// Context-aware filtering done in extractSentences() to avoid splitting on IPs/numbers/abbreviations
const SENTENCE_END_RE = /([。！？]|\n|[.!?])/g

function extractSentences(text: string): { sentences: string[]; remainder: string } {
  const sentences: string[] = []
  let working = text
  let lastEnd = 0
  const re = new RegExp(SENTENCE_END_RE.source, 'g')
  let match: RegExpExecArray | null

  while ((match = re.exec(working)) !== null) {
    const punct = match[1]
    const endIdx = match.index + match[0].length

    // Chinese punctuation (。！？) and newlines (\n): always split
    if (punct === '。' || punct === '！' || punct === '？' || punct === '\n') {
      const sentence = working.substring(lastEnd, endIdx).trim()
      if (sentence) sentences.push(sentence)
      lastEnd = endIdx
      re.lastIndex = lastEnd
      continue
    }

    // ! and ? are strong sentence enders — always split
    if (punct === '!' || punct === '?') {
      const sentence = working.substring(lastEnd, endIdx).trim()
      if (sentence) sentences.push(sentence)
      lastEnd = endIdx
      re.lastIndex = lastEnd
      continue
    }

    // --- Period (.) only: apply context checks ---

    const charBefore = match.index > 0 ? working[match.index - 1] : ''
    const afterPunct = working.substring(endIdx)
    const nextChar = afterPunct.match(/^(\S)/)?.[1] || ''
    const nextNonSpace = afterPunct.match(/^\s*(\S)/)?.[1] || ''

    // Rule 1: Don't split decimal points or dotted identifiers.
    // a) digit.digit — IPs (192.168.1.1), decimals (3.14)
    // b) digit followed IMMEDIATELY (no space) by any char — 4.7-star,
    //    3.5亿, 2.5x, 99.9%. A sentence-ending period is always followed
    //    by whitespace; no-space means this dot is glued to the number.
    if (/\d/.test(charBefore) && (/\d/.test(nextNonSpace) || nextChar)) {
      continue
    }

    // Rule 2: Don't split on known abbreviations (Dr., Mr., Mrs., etc.)
    const beforeMatch = working.substring(0, match.index).match(/(\w+)\.?$/)
    if (beforeMatch) {
      const word = beforeMatch[1].toLowerCase()
      if (ABBREVIATIONS.has(word)) {
        continue
      }
    }

    // Rule 3: Split only if followed by space + capital letter, or end of text
    // This catches genuine sentence boundaries and avoids splitting on things like filenames, URLs, etc.
    if (!nextNonSpace || /[A-Z\u00C0-\u024F]/.test(nextNonSpace)) {
      const sentence = working.substring(lastEnd, endIdx).trim()
      if (sentence) sentences.push(sentence)
      lastEnd = endIdx
      re.lastIndex = lastEnd
    }
    // Otherwise, this period is likely mid-sentence — skip it
  }

  const remainder = working.substring(lastEnd)
  return { sentences, remainder }
}

function enqueueSentence(sentence: string) {
  const trimmed = sentence.trim()
  if (!trimmed) return
  pendingSentences.push(trimmed)
}

function revealSegment(index: number) {
  const text = queuedSegmentText.get(index)
  if (!text) return
  queuedSegmentText.delete(index)
  messages.value.push({ id: nextMessageId++, role: 'ai', text })
  scrollToBottom()
}

function revealAllQueuedSegments() {
  for (const index of [...queuedSegmentText.keys()].sort((a, b) => a - b)) {
    revealSegment(index)
  }
}

async function flushPendingSegment(final: boolean) {
  if (pendingSentences.length === 0 && !final) return

  const sentences = pendingSentences.splice(0)
  let segmentIndex: number | null = null
  if (sentences.length > 0) {
    segmentIndex = nextQueuedSegmentIndex++
    queuedSegmentText.set(segmentIndex, sentences.join(''))
  }

  addDebugEntry('info', 'TTS', `Queueing staged TTS segment with ${sentences.length} sentences`)
  try {
    await invoke('speak_batch_queued', { texts: sentences, finalSegment: final })
  } catch (e) {
    console.error('speak_batch_queued failed:', e)
    addDebugEntry('error', 'TTS', 'speak_batch_queued failed', String(e))
    isTtsActive = false
    if (segmentIndex !== null) revealSegment(segmentIndex)
    resetStagedTtsState()
    await invoke('reset_tts_queue').catch(() => {})
  }
}

async function startBatchTTS() {
  if (pendingSentences.length === 0) {
    if (responseFinished && !isTtsActive) enterWakeMode()
    return
  }

  isTtsActive = true
  state.value = 'speaking'
  await flushPendingSegment(false)
}

async function maybeStagedFlush(force: boolean) {
  const silenceElapsed = pendingSentences.length > 0
    && Date.now() - lastDeltaAt >= STAGED_TTS_SILENCE_MS
  if (!force && pendingSentences.length < STAGED_TTS_SEGMENT_SIZE && !silenceElapsed) {
    return
  }

  if (stagedFlushTimer !== null) {
    clearTimeout(stagedFlushTimer)
    stagedFlushTimer = null
  }
  if (force) {
    if (pendingSentences.length > 0) {
      isTtsActive = true
      state.value = 'speaking'
      await flushPendingSegment(false)
    }
    return
  }
  await startBatchTTS()
}

function scrollToBottom() {
  nextTick(() => {
    if (chatArea.value) {
      chatArea.value.scrollTop = chatArea.value.scrollHeight
      // Double-check after browser layout to handle dynamic content
      requestAnimationFrame(() => {
        if (chatArea.value) {
          chatArea.value.scrollTop = chatArea.value.scrollHeight
        }
      })
      // Final safety net for large one-shot DOM updates (batch reveal of dozens
      // of sentences + lazily-rendered tool cards can keep growing scrollHeight
      // for a few hundred ms after layout).
      setTimeout(() => {
        if (chatArea.value) {
          chatArea.value.scrollTop = chatArea.value.scrollHeight
        }
      }, 300)
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

  // Sequential cleanup with proper awaits to avoid mic stream conflicts:
  // 1. Stop mic capture (must complete before wake word tries to open the mic)
  // 2. Wait for the cpal stream to be fully dropped
  // 3. Start wake word detection
  state.value = 'waiting'
  ;(async () => {
    try {
      await invoke('stop_listening')
    } catch (e) {
      console.warn('[VoiceChat] stop_listening during enterWakeMode:', e)
    }
    isListening.value = false

    // Give the OS a moment to fully release the audio device
    await new Promise((r) => setTimeout(r, 300))

    try {
      await invoke('start_wake_word', {
        accessKey: null,
        keyword: 'jarvis',
      })
    } catch (e) {
      console.error('start_wake_word failed:', e)
      addDebugEntry('error', 'WAKE', 'start_wake_word failed', String(e))
    }
  })()
}

let unlisteners: Array<() => void> = []

onMounted(async () => {
  // Load chat history from disk
  try {
    const turns = await invoke<Array<{ user_text: string; ai_text: string }>>('load_chat_history', {
      sessionId: sessionId.value,
    })
    for (const turn of turns) {
      messages.value.push({ id: nextMessageId++, role: 'user', text: turn.user_text })
      messages.value.push({ id: nextMessageId++, role: 'ai', text: turn.ai_text })
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
      await invoke('stop_wake_word').catch(() => {})
      // Wait for the VAD stream to be fully dropped before opening capture stream
      await new Promise((r) => setTimeout(r, 300))
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

    // Guard: a stray near-silent clip that arrives AFTER we already have a
    // real transcription in-flight must NOT kill the pending conversation
    // (observed 2026-08-24: a 3-minute Hermes query was aborted mid-flight
    // by an ambient-noise empty clip → "didn't catch that" + back to wake,
    // so the reply never reached TTS).
    if ((!text || text.startsWith('[')) && isProcessing) {
      console.log('[VoiceChat] stt:result → stray empty clip while processing, ignoring')
      addDebugEntry('info', 'STT', 'Stray empty clip during processing — ignored')
      return
    }

    if (text && !text.startsWith('[') && isProcessing) {
      console.log('[VoiceChat] stt:result → real clip while processing, ignoring')
      addDebugEntry('info', 'STT', 'Clip during processing — ignored')
      return
    }

    // Late straggler check: an STT result arriving right after TTS finished
    // is echo/residual buffer, not a new user utterance — drop it.
    if (text && !text.startsWith('[') && lastTtsCompleteAt > 0
        && Date.now() - lastTtsCompleteAt < 2000) {
      console.log('[VoiceChat] stt:result → late straggler after TTS complete, ignoring')
      addDebugEntry('info', 'STT', 'Late straggler after TTS complete — ignored')
      return
    }

    addDebugEntry('success', 'STT', `Transcribed`, `text="${text}"`)
    if (!text || text.startsWith('[')) {
      console.log(`[VoiceChat] stt:result → empty/failed, showing error`)
      addDebugEntry('warning', 'STT', 'Empty or failed transcription', `raw="${text}"`)
      messages.value.push({ id: nextMessageId++, role: 'ai', text: "Sorry, I didn't catch that." })
      scrollToBottom()
      // Must stop mic and re-enter wake mode — otherwise the still-running
      // mic capture will detect more noise and loop endlessly.
      invoke('stop_listening').catch(() => {})
      isListening.value = false
      enterWakeMode()
      return
    }

    isProcessing = true

    // Clear tool calls for new conversation round
    toolCalls.value = []

    // Stop any in-progress TTS
    if (stagedFlushTimer !== null) {
      clearTimeout(stagedFlushTimer)
      stagedFlushTimer = null
    }
    pendingSentences.length = 0
    resetStagedTtsState()
    isTtsActive = false
    sentenceBuffer.value = ''
    fullResponseText.value = ''
    responseFinished = false
    await invoke('reset_tts_queue').catch(() => {})

    // Add user message to chat history
    messages.value.push({ id: nextMessageId++, role: 'user', text })
    scrollToBottom()

    console.log(`[VoiceChat] → hermes_chat_stream: "${text}"`)
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

  // Streaming delta — accumulate into hidden buffer, do NOT reveal in chat yet.
  // Each flushed segment creates its own chat bubble when TTS queues it.
  const u4 = await listen<{ content: string }>('hermes:delta', (event) => {
    state.value = 'responding'

    // Accumulate into the hidden full-response buffer (used for history save)
    fullResponseText.value += event.payload.content

    addDebugEntry('info', 'API', `Delta: +${event.payload.content.length} chars`, event.payload.content.length > 100 ? event.payload.content.slice(0, 100) + '...' : event.payload.content)

    sentenceBuffer.value += event.payload.content
    const { sentences, remainder } = extractSentences(sentenceBuffer.value)
    for (const s of sentences) {
      enqueueSentence(s)
    }
    sentenceBuffer.value = remainder
    lastDeltaAt = Date.now()
    if (stagedFlushTimer !== null) clearTimeout(stagedFlushTimer)
    stagedFlushTimer = setTimeout(() => {
      stagedFlushTimer = null
      void maybeStagedFlush(false)
    }, STAGED_TTS_SILENCE_MS)
    void maybeStagedFlush(false)
    scrollToBottom()
  })

  const u5 = await listen<{ tool: string; status: string }>('hermes:tool', (event) => {
    const { tool, status } = event.payload
    addDebugEntry('info', 'API', `Tool: ${tool}`, `status=${status}`)

    // Track tool calls for carousel display
    if (status === 'started') {
      toolCalls.value.push({ tool, status: 'started' })
    } else {
      const existing = toolCalls.value.find(t => t.tool === tool && t.status === 'started')
      if (existing) {
        existing.status = status === 'error' ? 'error' : 'completed'
      } else {
        toolCalls.value.push({ tool, status: status === 'error' ? 'error' : 'completed' })
      }
    }
  })

  const u6 = await listen('hermes:finish', async () => {
    responseFinished = true
    if (stagedFlushTimer !== null) {
      clearTimeout(stagedFlushTimer)
      stagedFlushTimer = null
    }
    // Mark all active tools as completed
    for (const tc of toolCalls.value) {
      if (tc.status === 'started') {
        tc.status = 'completed'
      }
    }
    addDebugEntry('success', 'API', 'Stream finished', `Total AI message length: ${fullResponseText.value.length}`)

    // Flush any remaining text in the buffer as a final sentence
    if (sentenceBuffer.value.trim()) {
      enqueueSentence(sentenceBuffer.value)
      sentenceBuffer.value = ''
    }

    // Flush the tail before locating the current turn's final AI bubble.
    await maybeStagedFlush(true)

    // Save this turn to history using the full response text. Segment bubbles
    // are presentation-only and do not alter the persisted turn payload.
    const msgs = messages.value
    let lastUser: Message | undefined
    for (let i = msgs.length - 1; i >= 0; i--) {
      if (msgs[i].role === 'user') {
        lastUser = msgs[i]
        break
      }
    }
    if (lastUser) {
      invoke('save_chat_history', {
        sessionId: sessionId.value,
        userText: lastUser.text,
        aiText: fullResponseText.value,
      }).catch((e) => console.error('Failed to save chat history:', e))
    }

    // Enqueue the tail first, then atomically deliver the finish signal even
    // when the response ended exactly on a previous segment boundary.
    await flushPendingSegment(true)
    if (!isTtsActive) revealAllQueuedSegments()
  })

  const u7 = await listen<{ error: string }>('hermes:error', async (event) => {
    addDebugEntry('error', 'API', 'Hermes error', event.payload.error)
    const lastMsg = messages.value[messages.value.length - 1]
    if (lastMsg && lastMsg.role === 'ai') {
      lastMsg.text += `\n\nError: ${event.payload.error}`
    }
    responseFinished = true
    revealAllQueuedSegments()
    if (stagedFlushTimer !== null) {
      clearTimeout(stagedFlushTimer)
      stagedFlushTimer = null
    }
    resetStagedTtsState()
    await invoke('reset_tts_queue').catch(() => {})
    enterWakeMode()
  })

  // TTS batch complete — all sentences finished playing
  const u8 = await listen('tts:complete', () => {
    addDebugEntry('info', 'TTS', 'TTS queue drained — all staged sentences played')
    isTtsActive = false
    lastTtsCompleteAt = Date.now()
    revealAllQueuedSegments()
    // Pin chat to the latest content after playback finishes — the user must
    // end up looking at the final reply, not somewhere mid-history.
    scrollToBottom()
    if (responseFinished) {
      enterWakeMode()
    } else {
      scrollToBottom()
    }
  })

  const u12 = await listen<{ index: number }>('tts:segment-start', (event) => {
    revealSegment(event.payload.index)
  })

  const u13 = await listen('barge-in:detected', () => {
    addDebugEntry('info', 'TTS', 'Barge-in detected — playback interrupted')
    isTtsActive = false
    isProcessing = false
    responseFinished = false
    pendingSentences.length = 0
    resetStagedTtsState()
    sentenceBuffer.value = ''
    state.value = 'listening'
  })

  unlisteners = [u1, u2, u3, u4, u5, u6, u7, u8, u9, u10, u11, u12, u13]

  await invoke('set_barge_in_enabled', { enabled: bargeInEnabled.value })

  // Auto-start wake word detection on app launch
  enterWakeMode()
})

onUnmounted(() => {
  if (stagedFlushTimer !== null) clearTimeout(stagedFlushTimer)
  unlisteners.forEach((u) => u())
  invoke('stop_wake_word').catch(() => {})
})

async function startListening() {
  if (stagedFlushTimer !== null) {
    clearTimeout(stagedFlushTimer)
    stagedFlushTimer = null
  }
  sentenceBuffer.value = ''
  fullResponseText.value = ''
  pendingSentences.length = 0
  resetStagedTtsState()
  isTtsActive = false
  isProcessing = false
  responseFinished = false
  await invoke('reset_tts_queue').catch(() => {})
  console.log('[VoiceChat] startListening')
  addDebugEntry('info', 'STATE', '→ listening', 'Mic started')
  await invoke('start_listening')
  isListening.value = true
}

async function toggleListening() {
  if (isListening.value) {
    await invoke('stop_listening')
    isListening.value = false
    enterWakeMode()
  } else {
    // Stop wake word first — must await to ensure VAD stream is dropped
    await invoke('stop_wake_word')
    await new Promise((r) => setTimeout(r, 300))
    await startListening()
  }
}

async function startNewSession() {
  resetStagedTtsState()
  await invoke('reset_tts_queue').catch(() => {})
  await invoke('stop_listening').catch(() => {})
  await invoke('stop_wake_word').catch(() => {})

  isListening.value = false
  messages.value = []
  toolCalls.value = []
  pendingSentences.length = 0
  sentenceBuffer.value = ''
  fullResponseText.value = ''
  isTtsActive = false
  isProcessing = false
  responseFinished = false
  if (stagedFlushTimer !== null) {
    clearTimeout(stagedFlushTimer)
    stagedFlushTimer = null
  }

  const now = new Date().toISOString()
  sessionId.value = `voice-${now.slice(0, 10)}-${now.slice(11, 16).replace(':', '')}`
  state.value = 'idle'
  addDebugEntry('info', 'SESSION', 'Started new session', sessionId.value)

  // Voice users expect the wake listener to come back on its own.
  if (wakeEnabled.value) {
    enterWakeMode()
  }
}

async function sendText() {
  if (!userText.value.trim()) return

  const text = userText.value.trim()
  userText.value = ''

  resetStagedTtsState()

  // Stop wake word
  await invoke('stop_wake_word')
  await invoke('reset_tts_queue')
  if (stagedFlushTimer !== null) {
    clearTimeout(stagedFlushTimer)
    stagedFlushTimer = null
  }

  // Add user message to chat history
  messages.value.push({ id: nextMessageId++, role: 'user', text })
  scrollToBottom()

  sentenceBuffer.value = ''
  fullResponseText.value = ''
  pendingSentences.length = 0
  isTtsActive = false
  responseFinished = false
  toolCalls.value = []
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

async function toggleBargeIn() {
  bargeInEnabled.value = !bargeInEnabled.value
  await invoke('set_barge_in_enabled', { enabled: bargeInEnabled.value })
  addDebugEntry('info', 'SYSTEM', `Barge-in ${bargeInEnabled.value ? 'enabled' : 'disabled'}`)
}
</script>

<template>
  <div class="voice-chat">
    <div class="main-content">
      <header class="chat-header">
      <StateIndicator :state="state" :api-connected="apiConnected" :wake-mode="wakeMode" :wake-keyword="wakeKeyword" />
      <div class="header-buttons">
        <button class="btn-newchat" @click="startNewSession" title="New Session">✨</button>
        <button
          class="btn-debug"
          :class="{ active: debugVisible }"
          @click="debugVisible = !debugVisible"
          title="Toggle Debug Panel"
        >
          🐛
        </button>
        <button
          class="btn-barge-in"
          :class="{ active: bargeInEnabled }"
          @click="toggleBargeIn"
          :title="bargeInEnabled ? 'Barge-in ON' : 'Barge-in OFF'"
        >
          {{ bargeInEnabled ? '🗣️' : '🚫' }}
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
        v-for="msg in messages"
        :key="msg.id"
        :role="msg.role"
        :text="msg.text"
      />

      <ToolCarousel
        v-if="(state === 'thinking' || state === 'responding') && toolCalls.length > 0"
        :tool-calls="toolCalls"
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

.btn-wake,
.btn-barge-in {
  padding: 6px 10px;
  border-radius: 20px;
  border: 1px solid #4a4a8a;
  background: transparent;
  cursor: pointer;
  font-size: 16px;
  transition: all 0.2s;
  line-height: 1;
}

.btn-wake.active,
.btn-barge-in.active {
  border-color: #6c5ce7;
  background: rgba(108, 92, 231, 0.15);
}

.btn-wake:hover,
.btn-barge-in:hover {
  opacity: 0.9;
}

.btn-debug,
.btn-newchat {
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

.btn-debug:hover,
.btn-newchat:hover {
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
