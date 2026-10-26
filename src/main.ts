import { mount } from 'svelte';
import App from './App.svelte';
import './app.css';
import './dashboard.css';
import { applyTheme, readTheme } from './lib/theme';

applyTheme(readTheme());

const target = document.getElementById('app');

if (!target) {
  throw new Error('Elemento raiz da aplicação não encontrado.');
}

mount(App, { target });
