import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

const createLogger = (dev: boolean) => {
  return {
    t: (msg: string, ...args: unknown[]) => {
      dev ? console.trace(msg, ...args) : undefined;
    },
    d: (msg: string, ...args: unknown[]) => {
      dev ? console.debug(msg, ...args) : undefined;
    },
    i: (msg: string, ...args: unknown[]) => {
      dev ? console.info(msg, ...args) : undefined;
    },
    w: (msg: string, ...args: unknown[]) => {
      dev ? console.warn(msg, ...args) : undefined;
    },
    e: (msg: string, ...args: unknown[]) => {
      dev ? console.error(msg, ...args) : undefined;
    },
    f: (msg: string, ...args: unknown[]) => {
      dev ? console.error(msg, ...args) : undefined;
    },
  };
};

export const Log = createLogger(true);

export const formatTime = (seconds: number) => {
  const mins = Math.floor(seconds / 60);
  const secs = seconds % 60;
  return `${mins.toString().padStart(2, "0")}:${secs.toString().padStart(2, "0")}`;
};

