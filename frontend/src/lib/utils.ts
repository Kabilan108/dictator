import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

import {
	LogDebug,
	LogError,
	LogFatal,
	LogInfo,
	LogTrace,
	LogWarning,
} from "@wailsjs/runtime";

export function cn(...inputs: ClassValue[]) {
	return twMerge(clsx(inputs));
}

const createLogger = (dev: boolean) => {
	return {
		t: (msg: string, ...args: unknown[]) => {
			dev ? console.trace(msg, ...args) : undefined;
			LogTrace(`${msg}: ${JSON.stringify(args)}`);
		},
		d: (msg: string, ...args: unknown[]) => {
			dev ? console.debug(msg, ...args) : undefined;
			LogDebug(`${msg}: ${JSON.stringify(args)}`);
		},
		i: (msg: string, ...args: unknown[]) => {
			dev ? console.info(msg, ...args) : undefined;
			LogInfo(`${msg}: ${JSON.stringify(args)}`);
		},
		w: (msg: string, ...args: unknown[]) => {
			dev ? console.warn(msg, ...args) : undefined;
			LogWarning(`${msg}: ${JSON.stringify(args)}`);
		},
		e: (msg: string, ...args: unknown[]) => {
			dev ? console.error(msg, ...args) : undefined;
			LogError(`${msg}: ${JSON.stringify(args)}`);
		},
		f: (msg: string, ...args: unknown[]) => {
			dev ? console.error(msg, ...args) : undefined;
			LogFatal(`${msg}: ${JSON.stringify(args)}`);
		},
	};
};

export const Log = createLogger(true);
