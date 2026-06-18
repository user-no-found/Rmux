import { createRouter, createWebHistory } from 'vue-router'
import LoginView from '../views/LoginView.vue'
import TerminalView from '../views/TerminalView.vue'
import ConfigView from '../views/ConfigView.vue'

const routes = [
  { path: '/', redirect: '/login' },
  { path: '/login', name: 'login', component: LoginView },
  { path: '/terminal', name: 'terminal', component: TerminalView, meta: { requiresAuth: true } },
  { path: '/config', name: 'config', component: ConfigView, meta: { requiresAuth: true } },
]

const router = createRouter({
  history: createWebHistory(),
  routes,
})

router.beforeEach((to, from, next) => {
  const hasToken = !!sessionStorage.getItem('rmux_token')
  const hasUser = !!sessionStorage.getItem('rmux_auth_user')
  if (to.meta.requiresAuth && !hasToken && !hasUser) {
    next('/login')
  } else {
    next()
  }
})

export default router
