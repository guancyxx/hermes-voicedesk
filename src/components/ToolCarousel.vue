<script setup lang="ts">
import { ref, computed, watch, onUnmounted } from 'vue'

export interface ToolCall {
  tool: string
  status: 'started' | 'completed' | 'error'
}

const props = defineProps<{
  toolCalls: ToolCall[]
}>()

const currentIndex = ref(0)
let timer: ReturnType<typeof setInterval> | null = null

const activeTools = computed(() => props.toolCalls.filter(t => t.status === 'started'))
const doneTools = computed(() => props.toolCalls.filter(t => t.status !== 'started'))

// Build display list: active tools first (cycling priority), then recently completed
const displayList = computed(() => {
  const list: ToolCall[] = [...activeTools.value]
  // Add recently completed (last 3 done tools)
  const recent = doneTools.value.slice(-3)
  for (const t of recent) {
    list.push(t)
  }
  return list
})

const currentTool = computed(() => {
  if (displayList.value.length === 0) return null
  return displayList.value[currentIndex.value % displayList.value.length]
})

function startCycle() {
  stopCycle()
  if (displayList.value.length <= 1) return
  timer = setInterval(() => {
    currentIndex.value = (currentIndex.value + 1) % displayList.value.length
  }, 2000)
}

function stopCycle() {
  if (timer !== null) {
    clearInterval(timer)
    timer = null
  }
}

// Reset index when list changes
watch(() => displayList.value.length, (len) => {
  if (len === 0) {
    currentIndex.value = 0
    stopCycle()
  } else if (currentIndex.value >= len) {
    currentIndex.value = 0
  }
  if (len > 1) {
    startCycle()
  }
})

// Set up cycling on mount if needed
watch(displayList, (list) => {
  if (list.length > 1 && timer === null) {
    startCycle()
  } else if (list.length <= 1) {
    stopCycle()
  }
}, { immediate: true })

onUnmounted(() => stopCycle())
</script>

<template>
  <div v-if="currentTool" class="tool-carousel">
    <div class="tool-pill" :class="currentTool.status">
      <span class="tool-icon">{{ currentTool.status === 'started' ? '🔧' : currentTool.status === 'error' ? '❌' : '✅' }}</span>
      <span class="tool-name">{{ currentTool.tool }}</span>
      <span v-if="displayList.length > 1" class="tool-count">
        {{ currentIndex + 1 }}/{{ displayList.length }}
      </span>
    </div>
  </div>
</template>

<style scoped>
.tool-carousel {
  display: flex;
  justify-content: flex-start;
  padding: 4px 16px;
  overflow: hidden;
}

.tool-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 12px;
  border-radius: 20px;
  background: #1a1a2e;
  border: 1px solid #2a2a4a;
  font-size: 12px;
  color: #a0a0d0;
  animation: slideIn 0.3s ease-out;
  max-width: 100%;
  white-space: nowrap;
}

.tool-pill.started {
  border-color: rgba(108, 92, 231, 0.4);
  color: #b0a0f0;
  box-shadow: 0 0 8px rgba(108, 92, 231, 0.1);
}

.tool-pill.completed {
  border-color: rgba(46, 204, 113, 0.3);
  color: #5dde8e;
}

.tool-pill.error {
  border-color: rgba(231, 76, 60, 0.3);
  color: #e88;
}

.tool-icon {
  font-size: 11px;
  line-height: 1;
}

.tool-name {
  font-family: 'SF Mono', 'Fira Code', 'Cascadia Code', monospace;
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tool-count {
  font-size: 10px;
  color: #555;
  margin-left: 2px;
}

@keyframes slideIn {
  from {
    opacity: 0;
    transform: translateY(-6px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}
</style>
