<script setup lang="ts">
defineProps<{ active: boolean }>()
</script>

<template>
  <div class="audio-wave" :class="{ active }">
    <div class="wave-container">
      <div v-for="i in 32" :key="i" class="wave-bar" :style="{ animationDelay: `${i * 30}ms` }"></div>
    </div>
    <div v-if="active" class="wave-label">Listening...</div>
  </div>
</template>

<style scoped>
.audio-wave {
  padding: 24px 16px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  min-height: 80px;
}

.wave-container {
  display: flex;
  align-items: center;
  gap: 2px;
  height: 48px;
}

.wave-bar {
  width: 3px;
  background: #3a3a6a;
  border-radius: 2px;
  height: 8px;
  transition: background 0.3s;
}

.audio-wave.active .wave-bar {
  background: #6c5ce7;
  animation: wave 0.8s ease-in-out infinite alternate;
}

.wave-label {
  font-size: 13px;
  color: #6c5ce7;
  animation: pulse 2s ease-in-out infinite;
}

@keyframes wave {
  0% { height: 8px; }
  100% { height: 40px; }
}

@keyframes pulse {
  0%, 100% { opacity: 0.5; }
  50% { opacity: 1; }
}
</style>
