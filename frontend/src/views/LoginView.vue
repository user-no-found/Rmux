<template>
  <div class="login-page">
    <div class="login-wrapper">
      <div class="brand-header">
        <img class="logo" :src="iconUrl" alt="Rmux" />
        <h1>Rmux</h1>
        <p class="subtitle" v-if="mode === 'setup'">首次使用：请选择是否设置访问密码</p>
        <p class="subtitle" v-else-if="mode === 'login'">请输入访问密码</p>
        <p class="subtitle" v-else>正在进入终端...</p>
      </div>

      <div class="login-card">
        <div class="card-title">
          <span class="dot red"></span>
          <span class="dot yellow"></span>
          <span class="dot green"></span>
          <span>{{ mode === 'setup' ? '初始化系统.sh' : '进入终端.sh' }}</span>
        </div>
        <div v-if="mode === 'setup' && setupStep === 'choice'" class="login-form">
          <button type="button" class="btn-primary login-btn" @click="setupStep = 'password'" :disabled="loading">
            设置访问密码
          </button>
          <button type="button" class="btn-ghost full-btn" @click="skipPassword" :disabled="loading">
            不设置，直接进入
          </button>
          <p class="hint">选择不设置后，以后打开应用会直接进入终端。</p>
        </div>

        <form @submit.prevent="handleSubmit" class="login-form" v-else-if="mode === 'setup' || mode === 'login'">
          <div class="field">
            <span class="prompt">$</span>
            <input v-model="password" type="password" placeholder="输入访问密码" required autocomplete="current-password" />
          </div>

          <p v-if="mode === 'setup'" class="hint">以后打开应用需要输入此密码。</p>

          <Transition name="shake">
            <p v-if="error" class="error-msg">! {{ error }}</p>
          </Transition>

          <div class="actions">
            <button v-if="mode === 'setup'" type="button" class="btn-ghost" @click="setupStep = 'choice'" :disabled="loading">返回</button>
            <button type="submit" class="btn-primary login-btn" :disabled="loading">
              <span>{{ mode === 'setup' ? '保存密码并进入' : '进入终端' }}</span>
            </button>
          </div>
        </form>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import axios from 'axios'
import { API_BASE, assetUrl } from '../runtimeBase'

const router = useRouter()
const iconUrl = assetUrl('icon.png')
const mode = ref('loading') // loading, setup, login, public
const setupStep = ref('choice')
const password = ref('')
const error = ref('')
const loading = ref(false)

const handleLoginSuccess = (data) => {
  sessionStorage.setItem('rmux_token', data.token)
  sessionStorage.setItem('rmux_auth_user', JSON.stringify(data.user))
  router.push('/terminal')
}

const checkStatus = async () => {
  try {
    const res = await axios.get(`${API_BASE}/api/auth/status`)
    if (res.data.success) {
      const status = res.data.data.status
      if (status === 'public') {
        // 明确跳过模式：直接尝试提取身份进入
        await autoEnterPublic()
      } else {
        mode.value = status // setup or login
      }
    }
  } catch (e) {
    console.error('Check status failed', e)
    error.value = '无法连接服务器'
  }
}

const autoEnterPublic = async () => {
  try {
    const res = await axios.get(`${API_BASE}/api/auth/me`)
    if (res.data.success) {
      sessionStorage.setItem('rmux_auth_user', JSON.stringify(res.data.data))
      router.push('/terminal')
    } else {
      // 理论上 public 模式不应该失败，除非连 me 都拿不到
      mode.value = 'setup'
    }
  } catch (e) {
    mode.value = 'setup'
  }
}

onMounted(async () => {
  await checkStatus()
})

const skipPassword = async () => {
  loading.value = true
  error.value = ''
  try {
    const res = await axios.post(`${API_BASE}/api/auth/setup`, { password: null })
    if (res.data.success) {
      handleLoginSuccess(res.data.data)
    }
  } catch (e) {
    error.value = e.response?.data?.message || e.message || '操作失败'
  } finally {
    loading.value = false
  }
}

const handleSubmit = async () => {
  loading.value = true
  error.value = ''
  try {
    if (mode.value === 'setup') {
      if (password.value.length < 6) {
        error.value = '密码至少需要6位'
        return
      }
      const res = await axios.post(`${API_BASE}/api/auth/setup`, { password: password.value })
      if (res.data.success) {
        handleLoginSuccess(res.data.data)
      }
    } else if (mode.value === 'login') {
      const res = await axios.post(`${API_BASE}/api/auth/login`, { password: password.value })
      if (res.data.success) {
        handleLoginSuccess(res.data.data)
      }
    }
  } catch (e) {
    error.value = e.response?.data?.message || '操作失败'
  } finally {
    loading.value = false
  }
}
</script>

<style scoped>
.login-page {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 100vh;
  background:
    radial-gradient(circle at 72% 18%, rgba(65, 119, 152, 0.24), transparent 34%),
    linear-gradient(145deg, #050b11 0%, #0b1620 48%, #071019 100%);
  color: #f3f7fb;
}
.login-wrapper { width: min(380px, calc(100vw - 40px)); }
.brand-header { text-align: center; margin-bottom: 30px; }
.logo { width: 72px; height: 72px; border-radius: 18px; margin-bottom: 14px; box-shadow: 0 18px 45px rgba(0,0,0,0.38); }
.brand-header h1 { font-size: 34px; color: #f3f7fb; margin: 0; letter-spacing: 0; }
.subtitle { color: #a5b2bf; font-size: 14px; margin-top: 8px; min-height: 20px; }
.login-card { overflow: hidden; border-radius: 10px; border: 1px solid rgba(196,226,243,0.14); background: rgba(12, 23, 32, 0.9); box-shadow: 0 28px 70px rgba(0,0,0,0.38); }
.card-title { display: flex; align-items: center; gap: 9px; height: 42px; padding: 0 16px; color: #a5b2bf; border-bottom: 1px solid rgba(196,226,243,0.12); background: rgba(7, 16, 24, 0.78); font-family: Menlo, Monaco, "Courier New", monospace; font-size: 12px; }
.dot { width: 11px; height: 11px; border-radius: 50%; display: inline-block; }
.dot.red { background: #ff5f57; }
.dot.yellow { background: #ffbd2e; }
.dot.green { background: #28c840; }
.login-form { display: flex; flex-direction: column; gap: 20px; padding: 30px 24px 24px; }
.field { display: flex; align-items: center; gap: 10px; border: 1px solid rgba(196,226,243,0.14); border-radius: 8px; background: rgba(5, 13, 19, 0.74); padding: 0 14px; }
.prompt { color: #4ee45d; font-family: Menlo, Monaco, "Courier New", monospace; font-weight: 800; }
input { background: transparent; border: 0; padding: 13px 0; color: #fff; outline: none; width: 100%; font-size: 14px; }
input::placeholder { color: #657483; }
.error-msg { color: #ff7a86; font-size: 13px; background: rgba(255,95,109,0.1); padding: 10px; border-radius: 6px; }
.actions { display: flex; gap: 12px; margin-top: 2px; }
.login-btn { flex: 1; border: none; background: #4ee45d; color: #041008; padding: 13px; border-radius: 8px; font-weight: 800; cursor: pointer; }
.login-btn:disabled { opacity: 0.6; cursor: not-allowed; }
.btn-ghost { background: transparent; border: 1px solid rgba(196,226,243,0.14); color: #cbd5df; padding: 0 18px; border-radius: 8px; cursor: pointer; }
.full-btn { min-height: 44px; }
.btn-ghost:disabled { opacity: 0.6; cursor: not-allowed; }
.hint { font-size: 12px; color: #7f8d99; text-align: center; }
</style>
