/** @type {import('tailwindcss').Config} */
export default {
  // preflight 已开启（antd 卸载后不再有共存问题）：Tabs/Segmented 等无边框
  // 按钮此前漏出 UA 默认边框（真机走查实锤），依赖 base 重置归零。
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
