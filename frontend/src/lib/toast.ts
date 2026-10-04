import { writable } from 'svelte/store';

export type ToastType = 'success' | 'error' | 'info' | 'warn';

export interface Toast {
  id: number;
  message: string;
  type: ToastType;
}

const { subscribe, update } = writable<Toast[]>([]);

let idCounter = 0;

export const toasts = {
  subscribe,
  add: (message: string, type: ToastType = 'info', duration: number = 3000) => {
    const id = idCounter++;
    update(all => [...all, { id, message, type }]);
    
    if (duration > 0) {
      setTimeout(() => {
        update(all => all.filter(t => t.id !== id));
      }, duration);
    }
  },
  remove: (id: number) => {
    update(all => all.filter(t => t.id !== id));
  }
};
