import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MAX_PHOTOS, downscale, downscaleAll } from './photo';

// A recon is a dozen 12 MP frames on a phone whose browser kills the tab
// near half a gigabyte. The contract under test: EXIF orientation is
// honored explicitly (a sideways shelf is silent recon loss), the scale
// math never upscales, and decoding is strictly one frame at a time —
// peak memory is one full-resolution bitmap, not N.

let liveBitmaps: number;
let maxLiveBitmaps: number;
let bitmapOptions: ImageBitmapOptions | undefined;

function fakeBitmap(width: number, height: number) {
	liveBitmaps++;
	maxLiveBitmaps = Math.max(maxLiveBitmaps, liveBitmaps);
	return {
		width,
		height,
		close: () => {
			liveBitmaps--;
		}
	};
}

/** A canvas double that records its dimensions and encodes to a tiny blob. */
function fakeCanvas(record: { width?: number; height?: number }) {
	return {
		set width(w: number) {
			record.width = w;
		},
		get width() {
			return record.width ?? 0;
		},
		set height(h: number) {
			record.height = h;
		},
		get height() {
			return record.height ?? 0;
		},
		getContext: () => ({ drawImage: () => {} }),
		toBlob: (cb: (b: Blob | null) => void) =>
			cb({ arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer } as Blob)
	};
}

const dims = { current: { width: 4000, height: 3000 } };
const canvasDims: { width?: number; height?: number } = {};

beforeEach(() => {
	liveBitmaps = 0;
	maxLiveBitmaps = 0;
	bitmapOptions = undefined;
	vi.stubGlobal(
		'createImageBitmap',
		vi.fn(async (_file: File, options?: ImageBitmapOptions) => {
			bitmapOptions = options;
			return fakeBitmap(dims.current.width, dims.current.height);
		})
	);
	vi.stubGlobal('document', {
		createElement: (tag: string) => {
			expect(tag).toBe('canvas');
			return fakeCanvas(canvasDims);
		}
	});
});

afterEach(() => {
	vi.unstubAllGlobals();
});

const file = new File([new Uint8Array(16)], 'shelf.jpg', { type: 'image/jpeg' });

describe('downscale', () => {
	it('honors EXIF orientation explicitly and scales the long edge to fit', async () => {
		dims.current = { width: 4000, height: 3000 };
		const image = await downscale(file);
		expect(bitmapOptions?.imageOrientation).toBe('from-image');
		expect(canvasDims).toEqual({ width: 1568, height: 1176 });
		expect(image.media_type).toBe('image/jpeg');
		expect(image.data).toBe('AQID'); // base64 of [1,2,3]
		expect(liveBitmaps, 'bitmap closed').toBe(0);
	});

	it('never upscales a small frame', async () => {
		dims.current = { width: 800, height: 600 };
		await downscale(file);
		expect(canvasDims).toEqual({ width: 800, height: 600 });
	});
});

describe('downscaleAll', () => {
	it('decodes strictly one frame at a time', async () => {
		dims.current = { width: 4000, height: 3000 };
		const images = await downscaleAll([file, file, file]);
		expect(images).toHaveLength(3);
		expect(maxLiveBitmaps, 'one full-resolution bitmap live at a time').toBe(1);
	});
});

it('the pick cap matches what the server admits', () => {
	expect(MAX_PHOTOS).toBe(12);
});
