import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
// eslint-disable-next-line import/no-unassigned-import
import '@fontsource-variable/space-grotesk'
// eslint-disable-next-line import/no-unassigned-import
import '@fontsource-variable/hanken-grotesk'
// eslint-disable-next-line import/no-unassigned-import
import '@fontsource-variable/jetbrains-mono'
import './index.css'
import { App } from './app/App.tsx'

const rootElement = document.getElementById('root')
if (!rootElement) throw new Error('#root element not found')

createRoot(rootElement).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
