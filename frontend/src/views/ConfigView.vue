<template>
  <div class="config-page">
    <aside class="settings-sidebar">
      <div class="logo-row">
        <img :src="iconUrl" alt="" />
      </div>
      <button class="side-icon" title="返回终端" @click="router.push('/terminal')">⌘</button>
      <div class="side-spacer"></div>
      <button class="side-icon active" title="设置">⚙</button>
    </aside>

    <main class="settings-main">
      <header class="topbar">
        <div class="brand">Rmux <span class="sep">/</span> 设置</div>
        <button class="btn-ghost back-btn" @click="router.push('/terminal')">
          <span class="icon">←</span> 返回终端
        </button>
      </header>

      <div class="config-body">
        <nav class="config-nav">
        <button :class="['nav-item', {active: tab === 'theme'}]" @click="tab = 'theme'">
          <span class="nav-icon">>_</span> 外观设置
        </button>
        <button :class="['nav-item', {active: tab === 'danger'}]" @click="tab = 'danger'">
          <span class="nav-icon">!!</span> 危险区域
        </button>
      </nav>

      <main class="config-content">
        <!-- 主题 -->
        <Transition name="fade" mode="out-in">
          <div v-if="tab === 'theme'" class="section">
            <div class="section-header">
              <h3>终端外观设置</h3>
            </div>
            <div class="settings-card card">
              <div class="field">
                <label>颜色主题</label>
                <select v-model="themeSettings.theme">
                  <option value="onedark">Tokyo Night (One Dark)</option>
                  <option value="dracula">Dracula</option>
                  <option value="solarized">Solarized Dark</option>
                  <option value="nord">Nord</option>
                </select>
              </div>
              <div class="field">
                <label>字体大小 (px)</label>
                <input v-model.number="themeSettings.fontSize" type="number" min="10" max="30" />
              </div>
              <div class="field">
                <label>光标样式</label>
                <select v-model="themeSettings.cursorStyle">
                  <option value="block">方块 (Block)</option>
                  <option value="underline">下划线 (Underline)</option>
                  <option value="bar">竖线 (Bar)</option>
                </select>
              </div>
              <div class="actions">
                <button class="btn-primary" @click="saveTheme">应用更改</button>
              </div>
            </div>
          </div>

          <!-- 危险区域 -->
          <div v-else-if="tab === 'danger'" class="section">
            <div class="section-header">
              <h3>危险区域</h3>
            </div>
            <div class="settings-card card danger-card">
              <div class="danger-item">
                <div class="item-info">
                  <h4>清空应用数据</h4>
                  <p>删除所有日志、终端输出记录和背景图片。此操作不可撤销。</p>
                </div>
                <button class="btn-danger" @click="handlePurge">执行清空</button>
              </div>
              
              <div class="danger-item">
                <div class="item-info">
                  <h4>重置访问密码</h4>
                  <p>删除当前设置的访问密码。删除后应用将直接开放进入。</p>
                </div>
                <button class="btn-danger" @click="handleResetAuth">重置密码</button>
              </div>
            </div>
          </div>
        </Transition>
      </main>
      </div>
    </main>
  </div>
</template>

<script setup>
import { ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import axios from 'axios'
import { API_BASE } from '../runtimeBase'

const router = useRouter()
const iconUrl = '/icon.png'

// 状态
const tab = ref('theme')
const themeSettings = ref({ theme: 'onedark', fontSize: 14, cursorStyle: 'block' })

// 辅助函数
const auth = () => ({
  headers: sessionStorage.getItem('rmux_token')
    ? { Authorization: 'Bearer ' + sessionStorage.getItem('rmux_token') }
    : {}
})

const loadData = async () => {
  try {
    const res = await axios.get(`${API_BASE}/api/theme`, auth())
    if (res.data.success) Object.assign(themeSettings.value, res.data.data)
  } catch (e) {
    console.error('加载设置失败', e)
  }
}

const saveTheme = async () => {
  try {
    await axios.post(`${API_BASE}/api/theme`, themeSettings.value, auth())
    alert('主题设置已更新')
  } catch (e) {
    alert('保存失败')
  }
}

const handlePurge = async () => {
  if (!confirm('确定要删除所有应用数据吗？这将清空日志和输出历史。')) return
  try {
    await axios.post(`${API_BASE}/api/system/purge`, {}, auth())
    alert('数据已清空')
  } catch (e) {
    alert('清空失败')
  }
}

const handleResetAuth = async () => {
  // 这里暂时复用 purge 或单独写一个接口
  alert('暂未实现，请通过清空数据或手动删除 auth 文件执行')
}

onMounted(loadData)
</script>

<style scoped>
.config-page {
  min-height: 100vh;
  display: grid;
  grid-template-columns: 76px minmax(0, 1fr);
  background:
    radial-gradient(circle at 80% 10%, rgba(65, 119, 152, 0.2), transparent 32%),
    linear-gradient(145deg, #050b11 0%, #0b1620 48%, #071019 100%);
  color: #f3f7fb;
}
.settings-sidebar { display: flex; flex-direction: column; align-items: center; gap: 14px; padding: 16px 0; background: rgba(7, 17, 25, 0.84); border-right: 1px solid rgba(196,226,243,0.12); }
.logo-row img { width: 38px; height: 38px; border-radius: 9px; }
.side-icon { width: 42px; height: 42px; border: 0; border-radius: 9px; color: #cbd5df; background: transparent; cursor: pointer; }
.side-icon:hover, .side-icon.active { background: rgba(255,255,255,0.08); color: #4ee45d; }
.side-spacer { flex: 1; }
.settings-main { min-width: 0; }
.topbar { display: flex; align-items: center; justify-content: space-between; background: rgba(7, 16, 24, 0.78); padding: 0 24px; height: 58px; border-bottom: 1px solid rgba(196,226,243,0.12); }
.brand { font-weight: 800; font-size: 16px; color: #f3f7fb; }
.brand .sep { color: #657483; margin: 0 8px; font-weight: 300; }
.back-btn { font-size: 14px; display: flex; align-items: center; gap: 8px; color: #cbd5df; border: none; background: transparent; cursor: pointer; }
.config-body { display: flex; max-width: 1000px; margin: 0 auto; padding: 42px 24px; gap: 40px; }
.config-nav { display: flex; flex-direction: column; gap: 8px; min-width: 190px; }
.nav-item { background: transparent; border: none; text-align: left; padding: 12px 16px; border-radius: 8px; cursor: pointer; color: #a5b2bf; font-size: 14px; display: flex; align-items: center; gap: 12px; transition: all 0.2s; }
.nav-icon { font-family: Menlo, Monaco, "Courier New", monospace; color: #4ee45d; }
.nav-item:hover { background: rgba(255,255,255,0.06); color: #f3f7fb; }
.nav-item.active { background: rgba(22, 34, 45, 0.84); color: #fff; font-weight: 600; }
.config-content { flex: 1; min-width: 0; }
.section-header { margin-bottom: 24px; }
.section-header h3 { margin: 0; font-size: 20px; color: #f3f7fb; }
.settings-card { padding: 24px; background: rgba(12, 23, 32, 0.9); border-radius: 10px; border: 1px solid rgba(196,226,243,0.14); box-shadow: 0 28px 70px rgba(0,0,0,0.25); }
.field { margin-bottom: 20px; display: flex; flex-direction: column; gap: 8px; }
.field label { font-size: 12px; font-weight: 700; color: #a5b2bf; text-transform: uppercase; }
input, select { background: rgba(5, 13, 19, 0.74); border: 1px solid rgba(196,226,243,0.14); color: #fff; padding: 10px; border-radius: 8px; outline: none; }
.btn-primary { background: #4ee45d; color: #041008; border: none; padding: 10px 20px; border-radius: 8px; font-weight: 800; cursor: pointer; }
.btn-danger { background: #ff5f6d; color: #120407; border: none; padding: 8px 16px; border-radius: 8px; font-weight: 800; cursor: pointer; }
.danger-item { display: flex; justify-content: space-between; align-items: center; gap: 18px; padding: 16px 0; border-bottom: 1px solid rgba(196,226,243,0.12); }
.danger-item:last-child { border-bottom: none; }
.item-info h4 { margin: 0; color: #ff7a86; }
.item-info p { margin: 4px 0 0; font-size: 13px; color: #7f8d99; }
.actions { margin-top: 20px; display: flex; justify-content: flex-end; }
.fade-enter-active, .fade-leave-active { transition: opacity 0.2s ease; }
.fade-enter-from, .fade-leave-to { opacity: 0; }
</style>
