<script setup lang="ts">
defineProps<{
  text: string
  placeholder: string
}>()

const emit = defineEmits<{
  'update:text': [value: string]
  send: []
}>()

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    emit('send')
  }
}
</script>

<template>
  <div class="transcription-area">
    <textarea
      :value="text"
      :placeholder="placeholder"
      class="transcription-input"
      rows="2"
      @input="emit('update:text', ($event.target as HTMLTextAreaElement).value)"
      @keydown="onKeydown"
    ></textarea>
    <button class="send-btn" @click="emit('send')" :disabled="!text.trim()">
      ↑
    </button>
  </div>
</template>

<style scoped>
.transcription-area {
  display: flex;
  gap: 8px;
  padding: 12px 16px;
  align-items: flex-end;
}

.transcription-input {
  flex: 1;
  background: #222244;
  border: 1px solid #3a3a6a;
  border-radius: 12px;
  padding: 10px 14px;
  color: #e0e0e0;
  font-size: 14px;
  resize: none;
  outline: none;
  font-family: inherit;
  line-height: 1.4;
}

.transcription-input:focus {
  border-color: #6c5ce7;
}

.transcription-input::placeholder {
  color: #666;
}

.send-btn {
  width: 36px;
  height: 36px;
  border-radius: 50%;
  border: none;
  background: #6c5ce7;
  color: white;
  font-size: 18px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.send-btn:disabled {
  background: #3a3a6a;
  cursor: not-allowed;
}
</style>
