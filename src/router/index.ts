import { createRouter, createWebHistory } from 'vue-router'
import VoiceChat from '../views/VoiceChat.vue'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: '/',
      name: 'voice-chat',
      component: VoiceChat,
    },
  ],
})

export default router
