<script setup lang="ts">
defineProps<{
  state: string
  apiConnected: boolean
}>()

const stateLabels: Record<string, string> = {
  idle: 'Ready',
  listening: 'Listening...',
  thinking: 'Thinking...',
  responding: 'Responding...',
  speaking: 'Speaking...',
}
</script>

<template>
  <div class="state-indicator">
    <div class="state-pill" :class="state">
      <span class="state-dot"></span>
      {{ stateLabels[state] || state }}
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
.state-pill.listening .state-dot { background: #6c5ce7; animation: pulse 1s infinite; }
.state-pill.thinking .state-dot { background: #f39c12; animation: pulse 1s infinite; }
.state-pill.responding .state-dot { background: #3498db; }
.state-pill.speaking .state-dot { background: #2ecc71; }
</style>
