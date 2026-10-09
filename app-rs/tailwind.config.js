/** @type {import('tailwindcss').Config} */
export default {
  // preflight 关闭：迁移期与 AntD 共存，避免 base 重置打破存量页面；
  // 全量迁移完成后可开启。
  corePlugins: { preflight: false },
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  // 深色跟随应用主题（data-theme 由 applyTheme 写入，非系统偏好）
  darkMode: ['selector', '[data-theme="dark"]'],
  theme: {
    extend: {
      colors: {
        // 全部桥接 --sk-* 主题变量：换肤/深浅切换自动跟随，组件内禁止写死色值
        sk: {
          bg: 'var(--sk-bg)',
          card: 'var(--sk-card)',
          border: 'var(--sk-border)',
          glass: 'var(--sk-glass_border)',
          text: 'var(--sk-text)',
          sub: 'var(--sk-sub)',
          accent: 'var(--sk-accent)',
          'accent-hover': 'var(--sk-accent_hover)',
          'accent-soft': 'var(--sk-accent_soft)',
          'accent-soft2': 'var(--sk-accent_soft2)',
          ok: 'var(--sk-ok)',
          warn: 'var(--sk-warn)',
          err: 'var(--sk-err)',
          hover: 'var(--sk-hover)',
          shadow: 'var(--sk-shadow_color)',
        },
      },
      borderRadius: {
        sk: '10px',
        'sk-lg': '14px',
      },
      fontFamily: {
        ui: '"Segoe UI Variable", "Segoe UI", "Microsoft YaHei UI", system-ui, sans-serif',
      },
    },
  },
  plugins: [],
}
