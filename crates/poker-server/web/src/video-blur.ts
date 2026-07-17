/**
 * Google-Meet-style webcam BACKGROUND processing for the OUTGOING video track.
 *
 * Modes:
 *   - 'off'   : pass the raw camera track through unchanged (no processing).
 *   - 'blur'  : keep the person sharp, blur the background.
 *   - 'image' : keep the person sharp, replace the background with an image.
 *
 * HOW IT WORKS
 * ------------
 * A per-frame person/background segmentation mask is produced by MediaPipe's
 * ImageSegmenter (the "selfie segmenter" model). Each frame we:
 *   1. draw the raw camera frame to an offscreen <canvas> (2D ctx),
 *   2. draw a blurred (or image) copy of the same frame,
 *   3. composite the two using the segmentation mask so the foreground (person)
 *      comes from the sharp copy and the background from the blurred/image copy,
 *   4. expose the canvas as a MediaStream via canvas.captureStream().
 * That processed track is what media.ts hands to RTCRtpSender.replaceTrack(), so
 * the peer receives the blurred video; toggling modes swaps the composite without
 * renegotiating the WebRTC session.
 *
 * FULLY OFFLINE / STRICT-CSP
 * --------------------------
 * The MediaPipe WASM runtime AND the .tflite model are VENDORED into the repo
 * under web/public/mediapipe/ and served SAME-ORIGIN (see BASE below). Nothing is
 * fetched from a CDN or Google's servers, so the static bundle stays IPFS-hostable
 * under a CSP that blocks all external hosts.
 *
 * CSP CAVEAT: MediaPipe compiles its runtime from the bundled .wasm at load time
 * via WebAssembly.instantiate(). Under a strict CSP that means script-src must
 * include 'wasm-unsafe-eval' (this does NOT re-open eval() for JS — it only allows
 * WASM compilation). Same-origin connect-src must also allow the /mediapipe/* URLs
 * (covered by connect-src 'self'). No external host is ever contacted.
 *
 * GRACEFUL FALLBACK
 * -----------------
 * If the WASM/model fails to init (blocked, too weak, unsupported), start() and
 * setMode() reject/no-op and the caller keeps sending the RAW track — never a black
 * frame. The caller decides how to surface a small notice.
 */

// Dynamic import type only (keeps the heavy dep out of the initial chunk).
import type { ImageSegmenter } from '@mediapipe/tasks-vision'

export type BlurMode = 'off' | 'blur' | 'image'

// Same-origin base for the vendored wasm + model. Vite copies web/public/* to the
// static root verbatim, so these resolve to /mediapipe/... at runtime.
const BASE = '/mediapipe'
const MODEL_URL = `${BASE}/selfie_segmenter.tflite`
// Background blur strength (canvas filter). ~8-12px reads like Meet at 320x240.
const BLUR_PX = 10

export interface VideoBlur {
  /** the processed outgoing track (canvas capture). null until start(). */
  outputTrack: () => MediaStreamTrack | null
  /** current mode. */
  mode: () => BlurMode
  /**
   * Switch modes at runtime. 'off' stops the processing loop and the caller
   * should replaceTrack() back to the raw camera track. Resolves once applied.
   * Throws if the segmenter could not initialise (caller falls back to raw).
   */
  setMode: (m: BlurMode, source?: MediaStreamTrack) => Promise<void>
  /** provide/replace the background image (used by 'image' mode). */
  setBackgroundImage: (img: HTMLImageElement | ImageBitmap | null) => void
  /** true once MediaPipe initialised successfully. */
  ready: () => boolean
  /** tear everything down and release the model + canvas track. */
  stop: () => void
}

/**
 * Create a background-processing pipeline around a raw camera track.
 * Nothing is initialised until the first non-'off' setMode() call, so enabling
 * the camera with blur OFF costs nothing (no WASM, no model download).
 */
export function createVideoBlur(): VideoBlur {
  let segmenter: ImageSegmenter | null = null
  let initPromise: Promise<void> | null = null
  let isReady = false

  let currentMode: BlurMode = 'off'
  let sourceTrack: MediaStreamTrack | null = null
  let bgImage: HTMLImageElement | ImageBitmap | null = null

  // Processing surfaces.
  let video: HTMLVideoElement | null = null
  let canvas: HTMLCanvasElement | null = null
  let ctx: CanvasRenderingContext2D | null = null
  let maskCanvas: HTMLCanvasElement | null = null
  let maskCtx: CanvasRenderingContext2D | null = null
  let outStream: MediaStream | null = null
  let rafId: number | null = null
  let running = false

  async function ensureSegmenter(): Promise<void> {
    if (isReady) return
    if (initPromise) return initPromise
    initPromise = (async () => {
      // Lazy dynamic import → separate rollup chunk, not in the initial bundle.
      const vision = await import('@mediapipe/tasks-vision')
      const { FilesetResolver, ImageSegmenter } = vision
      // Points the runtime at the SAME-ORIGIN vendored wasm dir. No CDN.
      const fileset = await FilesetResolver.forVisionTasks(BASE)
      segmenter = await ImageSegmenter.createFromOptions(fileset, {
        baseOptions: {
          modelAssetPath: MODEL_URL,
          delegate: 'GPU', // WebGL-accelerated inference where available.
        },
        runningMode: 'VIDEO',
        outputCategoryMask: false,
        outputConfidenceMasks: true,
      })
      isReady = true
    })()
    try {
      await initPromise
    } catch (e) {
      initPromise = null
      isReady = false
      console.warn('[video-blur] segmenter init failed, will fall back to raw:', e)
      throw e
    }
  }

  function ensureSurfaces(w: number, h: number) {
    if (!canvas) {
      canvas = document.createElement('canvas')
      ctx = canvas.getContext('2d', { willReadFrequently: false })
    }
    if (!maskCanvas) {
      maskCanvas = document.createElement('canvas')
      maskCtx = maskCanvas.getContext('2d', { willReadFrequently: true })
    }
    if (canvas!.width !== w || canvas!.height !== h) {
      canvas!.width = w
      canvas!.height = h
      maskCanvas!.width = w
      maskCanvas!.height = h
    }
  }

  async function attachVideo(track: MediaStreamTrack): Promise<void> {
    if (!video) {
      video = document.createElement('video')
      video.muted = true
      video.playsInline = true
      video.autoplay = true
    }
    const s = new MediaStream([track])
    video.srcObject = s
    await video.play().catch(() => {})
    // Wait for real dimensions.
    if (!video.videoWidth) {
      await new Promise<void>((res) => {
        const onMeta = () => { video!.removeEventListener('loadedmetadata', onMeta); res() }
        video!.addEventListener('loadedmetadata', onMeta)
      })
    }
  }

  // The compositing step. mask is a Float32Array confidence map (1 = foreground).
  function composite(mask: Float32Array, w: number, h: number) {
    if (!ctx || !canvas || !video || !maskCtx || !maskCanvas) return

    // 1) Build an RGBA mask image (alpha = person confidence) on maskCanvas.
    const imgData = maskCtx.createImageData(w, h)
    const data = imgData.data
    for (let i = 0; i < mask.length; i++) {
      // person => opaque, background => transparent
      data[i * 4 + 3] = mask[i] > 0.5 ? 255 : Math.round(mask[i] * 255)
    }
    maskCtx.putImageData(imgData, 0, 0)

    // 2) Draw the BACKGROUND layer (blurred frame or image) at full canvas.
    ctx.save()
    ctx.filter = 'none'
    if (currentMode === 'image' && bgImage) {
      // cover-fit the image
      drawCover(ctx, bgImage, w, h)
    } else {
      // blurred copy of the current frame
      ctx.filter = `blur(${BLUR_PX}px)`
      ctx.drawImage(video, 0, 0, w, h)
      ctx.filter = 'none'
    }
    ctx.restore()

    // 3) Cut a person-shaped hole's inverse: draw the SHARP frame only where the
    //    mask is opaque. Use an offscreen composite: sharp frame masked by alpha.
    //    We do this by drawing the mask with 'destination-in' onto a temp of the
    //    sharp frame. To avoid a 3rd canvas, reuse maskCanvas: composite sharp
    //    frame into maskCanvas using source-in against the existing alpha mask.
    maskCtx.save()
    maskCtx.globalCompositeOperation = 'source-in'
    maskCtx.filter = 'none'
    maskCtx.drawImage(video, 0, 0, w, h)
    maskCtx.restore()

    // 4) Overlay the masked sharp foreground on top of the background layer.
    ctx.drawImage(maskCanvas, 0, 0, w, h)
  }

  function drawCover(
    c: CanvasRenderingContext2D,
    img: HTMLImageElement | ImageBitmap,
    w: number,
    h: number,
  ) {
    const iw = (img as HTMLImageElement).naturalWidth || (img as ImageBitmap).width
    const ih = (img as HTMLImageElement).naturalHeight || (img as ImageBitmap).height
    if (!iw || !ih) return
    const scale = Math.max(w / iw, h / ih)
    const dw = iw * scale
    const dh = ih * scale
    c.drawImage(img as CanvasImageSource, (w - dw) / 2, (h - dh) / 2, dw, dh)
  }

  function loop() {
    if (!running || !video || !segmenter || !canvas || !ctx) return
    const w = canvas.width
    const h = canvas.height
    if (video.readyState >= 2 && w && h) {
      try {
        const ts = performance.now()
        const result = segmenter.segmentForVideo(video, ts)
        const masks = result.confidenceMasks
        if (masks && masks[0]) {
          const mask = masks[0].getAsFloat32Array()
          composite(mask, w, h)
        } else {
          // No mask this frame: draw raw so we never emit black.
          ctx.drawImage(video, 0, 0, w, h)
        }
        result.close()
      } catch (e) {
        // Any per-frame failure: fall back to raw frame, keep looping.
        ctx.drawImage(video, 0, 0, w, h)
      }
    }
    rafId = requestAnimationFrame(loop)
  }

  function startLoop() {
    if (running) return
    running = true
    rafId = requestAnimationFrame(loop)
  }

  function stopLoop() {
    running = false
    if (rafId != null) {
      cancelAnimationFrame(rafId)
      rafId = null
    }
  }

  async function setMode(m: BlurMode, source?: MediaStreamTrack): Promise<void> {
    if (source) sourceTrack = source
    currentMode = m

    if (m === 'off') {
      stopLoop()
      return
    }
    if (!sourceTrack) throw new Error('[video-blur] no source track')

    // Init the model (may throw → caller falls back to the raw track).
    await ensureSegmenter()

    // Wire up the video element + canvas dimensions from the source track.
    await attachVideo(sourceTrack)
    const settings = sourceTrack.getSettings()
    const w = video!.videoWidth || settings.width || 320
    const h = video!.videoHeight || settings.height || 240
    ensureSurfaces(w, h)

    if (!outStream) {
      // captureStream at the source frame rate (or 30 default).
      const fps = settings.frameRate || 30
      outStream = canvas!.captureStream(fps)
    }
    startLoop()
  }

  function setBackgroundImage(img: HTMLImageElement | ImageBitmap | null) {
    bgImage = img
  }

  function outputTrack(): MediaStreamTrack | null {
    return outStream?.getVideoTracks()[0] ?? null
  }

  function stop() {
    stopLoop()
    outStream?.getTracks().forEach((t) => t.stop())
    outStream = null
    if (video) {
      video.srcObject = null
      video = null
    }
    canvas = null
    ctx = null
    maskCanvas = null
    maskCtx = null
    try {
      segmenter?.close()
    } catch { /* ignore */ }
    segmenter = null
    initPromise = null
    isReady = false
    currentMode = 'off'
    sourceTrack = null
  }

  return {
    outputTrack,
    mode: () => currentMode,
    setMode,
    setBackgroundImage,
    ready: () => isReady,
    stop,
  }
}
