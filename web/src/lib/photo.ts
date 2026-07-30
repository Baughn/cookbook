// Client-side photo downscaling: phone cameras produce 12 MP originals,
// the model reads shelves fine at ~1500 px, and store basements (M7) have
// terrible signal. Everything uploads as JPEG regardless of source format.

import type { ChatImage } from './api';

const MAX_EDGE = 1568;
const QUALITY = 0.85;

export async function downscale(file: File): Promise<ChatImage> {
	const bitmap = await createImageBitmap(file);
	try {
		const scale = Math.min(1, MAX_EDGE / Math.max(bitmap.width, bitmap.height));
		const canvas = document.createElement('canvas');
		canvas.width = Math.max(1, Math.round(bitmap.width * scale));
		canvas.height = Math.max(1, Math.round(bitmap.height * scale));
		const ctx = canvas.getContext('2d');
		if (!ctx) throw new Error('no canvas 2d context');
		ctx.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
		const dataUrl = canvas.toDataURL('image/jpeg', QUALITY);
		return { media_type: 'image/jpeg', data: dataUrl.slice(dataUrl.indexOf(',') + 1) };
	} finally {
		bitmap.close();
	}
}
