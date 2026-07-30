<script setup lang="ts">
defineProps<{
  text: string
  toolCalls: Array<{ tool: string; status: string }>
  state: string
}>()
</script>

<template>
  <div class="response-card">
    <div v-if="state === 'thinking'" class="thinking-indicator">
      <span class="dot"></span>
      <span class="dot"></span>
      <span class="dot"></span>
      <span class="thinking-text">Thinking...</span>
    </div>

    <div v-if="text" class="response-text" v-html="text"></div>

    <div v-if="toolCalls.length > 0" class="tool-calls">
      <div
        v-for="(call, i) in toolCalls"
        :key="i"
        class="tool-badge"
        :class="call.status"
      >
        {{ call.status === 'started' ? '🔧' : '✅' }} {{ call.tool }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.response-card {
  flex: 1;
  overflow-y: auto;
  padding: 16px;
}

.thinking-indicator {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 12px 0;
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
  animation: pulse 2s infinite;
}

.response-text {
  font-size: 15px;
  line-height: 1.6;
  color: #e0e0e0;
  white-space: pre-wrap;
  word-break: break-word;
}

.tool-calls {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 12px;
}

.tool-badge {
  padding: 4px 10px;
  border-radius: 12px;
  font-size: 12px;
  background: #2a2a4a;
  color: #a0a0d0;
}

.tool-badge.completed {
  background: #1a3a1a;
  color: #2ecc71;
}

@keyframes bounce {
  0%, 100% { transform: translateY(0); }
  50% { transform: translateY(-8px); }
}
</style>
