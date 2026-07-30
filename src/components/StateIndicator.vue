<script setup lang="ts">
defineProps<{
  state: string
  apiConnected: boolean
  wakeMode?: string
  wakeKeyword?: string
}>()

const stateLabels: Record<string, string> = {
  idle: 'Ready',
  waiting: 'Waiting...',
  listening: 'Listening...',
  transcribing: 'Transcribing...',
  thinking: 'Thinking...',
  responding: 'Responding...',
  speaking: 'Speaking...',
}
</script>

<template>
  <div class="state-indicator">
    <div class="state-pill" :class="state">
      <span class="state-dot"></span>
      <span>{{ stateLabels[state] || state }}</span>
      <span v-if="state === 'waiting' && wakeKeyword" class="wake-subtitle">
        "{{ wakeKeyword }}"
      </span>
    </div>
  </div>
</template>

<style scoped>
.state-indicator {
  display: flex;
  align-items: center;
}

.state-pill {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 12px;
  border-radius: 12px;
  font-size: 13px;
  background: #2a2a4a;
  color: #a0a0d0;
}

.state-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #666;
}

.state-pill.idle .state-dot { background: #888; }
.state-pill.waiting .state-dot { background: #6c5ce7; animation: wake-pulse 2s infinite; }
.state-pill.listening .state-dot { background: #6c5ce7; animation: pulse 1s infinite; }
.state-pill.thinking .state-dot { background: #f39c12; animation: pulse 1s infinite; }
.state-pill.transcribing .state-dot { background: #e67e22; animation: pulse 1s infinite; }
.state-pill.responding .state-dot { background: #3498db; }
.state-pill.speaking .state-dot { background: #2ecc71; }

.wake-subtitle {
  font-size: 11px;
  color: #888;
  font-style: italic;
}

@keyframes wake-pulse {
  0%, 100% { opacity: 0.4; box-shadow: 0 0 4px rgba(108, 92, 231, 0.3); }
  50% { opacity: 1; box-shadow: 0 0 12px rgba(108, 92, 231, 0.6); }
}
</style>
