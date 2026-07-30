<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  active: boolean
  volume?: number
}>()

const barCount = 32
const bars = computed(() => {
  const v = props.active ? (props.volume || 0) : 0
  return Array.from({ length: barCount }, (_, i) => {
    // Create a varied pattern based on volume
    const base = Math.sin((i / barCount) * Math.PI) * 0.6 + 0.2
    const noise = Math.random() * 0.4
    return Math.min((base + noise) * (0.1 + v * 3), 1.0)
  })
})
</script>

<template>
  <div class="audio-wave" :class="{ active }">
    <div class="wave-container">
      <div
        v-for="(h, i) in bars"
        :key="i"
        class="wave-bar"
        :style="{ height: `${Math.max(h * 48, 4)}px` }"
      ></div>
    </div>
    <div v-if="active" class="wave-label">Listening...</div>
  </div>
</template>

<style scoped>
.audio-wave {
  padding: 20px 16px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  min-height: 70px;
}

.wave-container {
  display: flex;
  align-items: flex-end;
  gap: 2px;
  height: 48px;
}

.wave-bar {
  width: 3px;
  background: #3a3a6a;
  border-radius: 2px;
  transition: height 0.05s ease, background 0.3s;
}

.audio-wave.active .wave-bar {
  background: #6c5ce7;
}

.wave-label {
  font-size: 13px;
  color: #6c5ce7;
  animation: pulse 2s ease-in-out infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 0.4; }
  50% { opacity: 1; }
}
</style>
