<script setup lang="ts">
defineProps<{
  role: 'user' | 'ai'
  text: string
}>()
</script>

<template>
  <div class="chat-bubble-wrapper" :class="role">
    <div class="chat-bubble" :class="role">
      <div class="bubble-text">{{ text }}</div>
    </div>
  </div>
</template>

<style scoped>
.chat-bubble-wrapper {
  display: flex;
  padding: 6px 16px;
}

.chat-bubble-wrapper.user {
  justify-content: flex-end;
}

.chat-bubble-wrapper.ai {
  justify-content: flex-start;
}

.chat-bubble {
  max-width: 80%;
  padding: 10px 14px;
  border-radius: 16px;
  font-size: 14px;
  line-height: 1.5;
  word-break: break-word;
  white-space: pre-wrap;
}

.chat-bubble.user {
  background: #6c5ce7;
  color: #ffffff;
  border-bottom-right-radius: 4px;
}

.chat-bubble.ai {
  background: #2a2a4a;
  color: #e0e0e0;
  border-bottom-left-radius: 4px;
}

.bubble-text {
  white-space: pre-wrap;
  text-align: left;
}
</style>
