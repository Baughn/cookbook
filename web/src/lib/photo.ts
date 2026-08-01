// Client-side photo downscaling: phone cameras produce 12 MP originals,
// the model reads shelves fine at ~1500 px, and store basements (M7) have
// terrible signal. Everything uploads as JPEG regardless of source format.

import type { ChatImage } from './api';

const MAX_EDGE = 1568;
const QUALITY = 0.85;

/// The server admits at most this many frames per exchange; the composer
/// caps picks at the same number so nothing is downscaled, uploaded and
/// then refused.
export const MAX_PHOTOS = 12;

export async function downscale(file: File): Promise<ChatImage> {
	// from-image explicitly: engines flipped this default at different
	// times, and where it is 'none' the canvas re-encode makes the
	// sideways rotation permanent — silent recon loss on the surface
	// whose whole point is reading a shelf.
	const bitmap = await createImageBitmap(file, { imageOrientation: 'from-image' });
	try {
		const scale = Math.min(1, MAX_EDGE / Math.max(bitmap.width, bitmap.height));
		const canvas = document.createElement('canvas');
		canvas.width = Math.max(1, Math.round(bitmap.width * scale));
		canvas.height = Math.max(1, Math.round(bitmap.height * scale));
		const ctx = canvas.getContext('2d');
		if (!ctx) throw new Error('no canvas 2d context');
		ctx.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
		const blob = await new Promise<Blob>((resolve, reject) =>
			canvas.toBlob(
				(b) => (b ? resolve(b) : reject(new Error('jpeg encode failed'))),
				'image/jpeg',
				QUALITY
			)
		);
		return { media_type: 'image/jpeg', data: base64(await blob.arrayBuffer()) };
	} finally {
		bitmap.close();
	}
}

/// Strictly one frame at a time: a recon is a dozen 12 MP picks, and N
/// full-resolution bitmaps at once (~48 MB each as RGBA) is how a phone
/// browser kills the tab — on the send path that has already consumed
/// the picks.
export async function downscaleAll(files: File[]): Promise<ChatImage[]> {
	const images: ChatImage[] = [];
	for (const file of files) {
		images.push(await downscale(file));
	}
	return images;
}

function base64(buffer: ArrayBuffer): string {
	const bytes = new Uint8Array(buffer);
	let binary = '';
	const CHUNK = 0x8000; // String.fromCharCode's argument limit
	for (let i = 0; i < bytes.length; i += CHUNK) {
		binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
	}
	return btoa(binary);
}
