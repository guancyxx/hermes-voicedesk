<script setup lang="ts">
import { ref, watch, nextTick } from 'vue'

export interface DebugEntry {
  timestamp: string
  type: 'info' | 'success' | 'warning' | 'error'
  category: string
  message: string
  detail?: string
}

const props = defineProps<{
  entries: DebugEntry[]
  visible: boolean
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'clear'): void
}>()

const logContainer = ref<HTMLElement | null>(null)

// Auto-scroll to bottom when new entries arrive
watch(
  () => props.entries.length,
  () => {
    nextTick(() => {
      if (logContainer.value) {
        logContainer.value.scrollTop = logContainer.value.scrollHeight
      }
    })
  }
)

function typeIcon(type: string): string {
  switch (type) {
    case 'success': return '✓'
    case 'warning': return '⚠'
    case 'error': return '✗'
    default: return '●'
  }
}
</script>

<template>
  <Transition name="slide">
    <div v-if="visible" class="debug-panel">
      <div class="debug-header">
        <span class="debug-title">🐛 Debug Log</span>
        <div class="debug-actions">
          <button class="debug-btn" title="Clear log" @click="emit('clear')">🗑</button>
          <button class="debug-btn" title="Close panel" @click="emit('close')">✕</button>
        </div>
      </div>

      <div ref="logContainer" class="debug-log">
        <div v-if="entries.length === 0" class="debug-empty">
          Waiting for events...
        </div>
        <div
          v-for="(entry, i) in entries"
          :key="i"
          class="debug-entry"
          :class="`entry-${entry.type}`"
        >
          <span class="entry-time">{{ entry.timestamp }}</span>
          <span class="entry-category">{{ entry.category }}</span>
          <span class="entry-icon">{{ typeIcon(entry.type) }}</span>
          <span class="entry-msg">{{ entry.message }}</span>
          <span v-if="entry.detail" class="entry-detail">{{ entry.detail }}</span>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.debug-panel {
  width: 380px;
  min-width: 380px;
  flex-shrink: 0;
  background: #0d0d1a;
  border-left: 1px solid #2a2a4a;
  display: flex;
  flex-direction: column;
  box-shadow: -4px 0 24px rgba(0, 0, 0, 0.5);
  overflow: hidden;
}

.debug-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  border-bottom: 1px solid #2a2a4a;
  background: #12122a;
  flex-shrink: 0;
}

.debug-title {
  font-size: 13px;
  font-weight: 600;
  color: #a0a0d0;
}

.debug-actions {
  display: flex;
  gap: 4px;
}

.debug-btn {
  padding: 4px 8px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: #888;
  cursor: pointer;
  font-size: 14px;
  line-height: 1;
  transition: all 0.15s;
}

.debug-btn:hover {
  background: rgba(255, 255, 255, 0.08);
  color: #ccc;
}

.debug-log {
  flex: 1;
  overflow-y: auto;
  padding: 8px 0;
  font-family: 'SF Mono', 'Fira Code', 'Cascadia Code', monospace;
  font-size: 11px;
  line-height: 1.5;
}

.debug-log::-webkit-scrollbar {
  width: 4px;
}

.debug-log::-webkit-scrollbar-track {
  background: transparent;
}

.debug-log::-webkit-scrollbar-thumb {
  background: #2a2a4a;
  border-radius: 2px;
}

.debug-empty {
  padding: 24px 14px;
  color: #555;
  text-align: center;
  font-style: italic;
}

.debug-entry {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  align-items: baseline;
  padding: 3px 14px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.02);
  transition: background 0.1s;
}

.debug-entry:hover {
  background: rgba(255, 255, 255, 0.03);
}

.entry-time {
  color: #555;
  flex-shrink: 0;
  font-size: 10px;
}

.entry-category {
  color: #777;
  flex-shrink: 0;
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.3px;
}

.entry-icon {
  flex-shrink: 0;
  width: 14px;
  text-align: center;
  font-size: 10px;
}

.entry-msg {
  color: #ccc;
  word-break: break-word;
}

.entry-detail {
  width: 100%;
  padding-left: 14px;
  color: #666;
  font-size: 10px;
  word-break: break-all;
  white-space: pre-wrap;
}

/* Type colors */
.entry-info .entry-icon,
.entry-info .entry-msg {
  color: #7eb8da;
}

.entry-success .entry-icon,
.entry-success .entry-msg {
  color: #7ecb8a;
}

.entry-warning .entry-icon,
.entry-warning .entry-msg {
  color: #e5c07b;
}

.entry-error .entry-icon,
.entry-error .entry-msg {
  color: #e06c75;
}

.entry-error {
  background: rgba(224, 108, 117, 0.06);
}

/* Slide transition */
.slide-enter-active,
.slide-leave-active {
  transition: transform 0.25s ease;
}

.slide-enter-from,
.slide-leave-to {
  transform: translateX(100%);
}
</style>
