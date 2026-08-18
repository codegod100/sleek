package uk.nandi.sleek;

import android.app.Activity;
import android.content.Context;
import android.content.pm.PackageManager;
import android.graphics.ImageFormat;
import android.hardware.camera2.CameraAccessException;
import android.hardware.camera2.CameraCaptureSession;
import android.hardware.camera2.CameraCharacteristics;
import android.hardware.camera2.CameraDevice;
import android.hardware.camera2.CameraManager;
import android.hardware.camera2.CaptureRequest;
import android.media.Image;
import android.media.ImageReader;
import android.os.Build;
import android.os.Handler;
import android.os.HandlerThread;
import android.util.Log;
import android.util.Size;
import android.view.Display;
import android.view.Surface;

import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Camera2 capture for NativeActivity AV calls.
 *
 * <p>Opens a camera (front preferred), streams {@link ImageFormat#YUV_420_888}
 * into an {@link ImageReader}, and pushes NV12 planes into Rust via JNI so
 * iroh-live can publish H.264 on the MoQ media plane.
 *
 * <p>ImageReader buffers are in <b>sensor</b> coordinates. Most phone sensors
 * sit at 90°/270°, so without a CW rotate by {@link
 * CameraCharacteristics#SENSOR_ORIENTATION} ± display rotation the published
 * video (and local preview) looks sideways. Rotation degrees are passed to
 * Rust, which rotates the NV12 planes before encode/preview.
 *
 * <p>Compiled into {@code sleek_activity.dex} (APK {@code classes.dex}). Rust
 * must resolve this class via {@code Activity.getClassLoader().loadClass}
 * — {@code FindClass} from a native worker thread uses the system loader and
 * cannot see APK classes.
 */
public final class CameraCapture {
    private static final String TAG = "SleekCamera";
    /** Target capture size — encoder preset is 360p; 640x480 is widely supported. */
    private static final int TARGET_WIDTH = 640;
    private static final int TARGET_HEIGHT = 480;

    private static final Object LOCK = new Object();
    private static CameraDevice cameraDevice;
    private static CameraCaptureSession captureSession;
    private static ImageReader imageReader;
    private static HandlerThread backgroundThread;
    private static Handler backgroundHandler;
    private static volatile boolean running;
    private static long frameCount;
    /** Host activity — used to read live display rotation while capturing. */
    private static volatile Activity hostActivity;
    private static int sensorOrientationDeg = 90;
    private static boolean frontFacing = true;

    static {
        // NativeActivity already loads libsleek.so; loadLibrary is a no-op when
        // the lib is present, and ensures JNI natives resolve if this class is
        // touched before the activity's meta-data load completes.
        try {
            System.loadLibrary("sleek");
        } catch (UnsatisfiedLinkError e) {
            Log.w(TAG, "libsleek not loaded yet; natives resolve after NativeActivity init", e);
        }
    }

    private CameraCapture() {}

    /**
     * Native: push one NV12 frame into the MoQ publish pipeline.
     *
     * @param rotationDegrees CW degrees (0/90/180/270) to apply so the frame
     *     is upright for the current display — see {@link #computeRotationDegrees()}.
     */
    private static native void onNv12Frame(
            byte[] yData,
            byte[] uvData,
            int width,
            int height,
            int yStride,
            int uvStride,
            int rotationDegrees);

    /** Native: camera opened successfully (or failed). */
    private static native void onCameraState(boolean opened, String detail);

    /**
     * List available cameras as {@code "id\\tlabel"} lines (id then tab then label).
     * Facing preference: front first, then back, then external/other.
     */
    public static String[] listCameras(Context context) {
        try {
            CameraManager manager =
                    (CameraManager) context.getSystemService(Context.CAMERA_SERVICE);
            if (manager == null) {
                return new String[0];
            }
            String[] ids = manager.getCameraIdList();
            List<String> front = new ArrayList<>();
            List<String> back = new ArrayList<>();
            List<String> other = new ArrayList<>();
            for (String id : ids) {
                CameraCharacteristics chars = manager.getCameraCharacteristics(id);
                Integer facing = chars.get(CameraCharacteristics.LENS_FACING);
                String label;
                if (facing != null && facing == CameraCharacteristics.LENS_FACING_FRONT) {
                    label = "Front camera · " + id;
                    front.add(id + "\t" + label);
                } else if (facing != null && facing == CameraCharacteristics.LENS_FACING_BACK) {
                    label = "Back camera · " + id;
                    back.add(id + "\t" + label);
                } else {
                    label = "Camera · " + id;
                    other.add(id + "\t" + label);
                }
            }
            List<String> out = new ArrayList<>(front.size() + back.size() + other.size());
            out.addAll(front);
            out.addAll(back);
            out.addAll(other);
            return out.toArray(new String[0]);
        } catch (Throwable t) {
            Log.w(TAG, "listCameras failed", t);
            return new String[0];
        }
    }

    /**
     * Start capture. {@code cameraId} may be null/empty (prefer front), a device
     * id, or the tokens {@code "front"} / {@code "back"}.
     */
    public static void start(final Activity activity, final String cameraId) {
        if (activity == null) {
            onCameraState(false, "null activity");
            return;
        }
        activity.runOnUiThread(
                new Runnable() {
                    @Override
                    public void run() {
                        startOnThread(activity, cameraId);
                    }
                });
    }

    /** Stop capture and release camera resources. Safe to call repeatedly. */
    public static void stop(final Activity activity) {
        Runnable r =
                new Runnable() {
                    @Override
                    public void run() {
                        stopLocked();
                    }
                };
        if (activity != null) {
            activity.runOnUiThread(r);
        } else {
            r.run();
        }
    }

    private static void startOnThread(Activity activity, String cameraId) {
        synchronized (LOCK) {
            stopLocked();
            if (activity.checkSelfPermission(android.Manifest.permission.CAMERA)
                    != PackageManager.PERMISSION_GRANTED) {
                Log.e(TAG, "CAMERA permission not granted");
                onCameraState(false, "permission denied");
                return;
            }
            CameraManager manager =
                    (CameraManager) activity.getSystemService(Context.CAMERA_SERVICE);
            if (manager == null) {
                onCameraState(false, "no CameraManager");
                return;
            }
            String id;
            try {
                id = resolveCameraId(manager, cameraId);
            } catch (CameraAccessException e) {
                Log.e(TAG, "resolveCameraId", e);
                onCameraState(false, e.getMessage());
                return;
            }
            if (id == null) {
                onCameraState(false, "no camera");
                return;
            }

            hostActivity = activity;
            try {
                CameraCharacteristics chars = manager.getCameraCharacteristics(id);
                Integer so = chars.get(CameraCharacteristics.SENSOR_ORIENTATION);
                sensorOrientationDeg = so != null ? so : 90;
                Integer facing = chars.get(CameraCharacteristics.LENS_FACING);
                frontFacing =
                        facing != null && facing == CameraCharacteristics.LENS_FACING_FRONT;
            } catch (CameraAccessException e) {
                Log.w(TAG, "characteristics; using default orientation", e);
                sensorOrientationDeg = 90;
                frontFacing = true;
            }

            Size size = chooseSize(manager, id);
            int width = size.getWidth();
            int height = size.getHeight();
            int rot = computeRotationDegrees();
            Log.i(
                    TAG,
                    "opening camera id="
                            + id
                            + " size="
                            + width
                            + "x"
                            + height
                            + " sensorOrient="
                            + sensorOrientationDeg
                            + " front="
                            + frontFacing
                            + " rot="
                            + rot);

            startBackgroundThread();
            frameCount = 0;
            imageReader = ImageReader.newInstance(width, height, ImageFormat.YUV_420_888, 2);
            imageReader.setOnImageAvailableListener(
                    new ImageReader.OnImageAvailableListener() {
                        @Override
                        public void onImageAvailable(ImageReader reader) {
                            Image image = null;
                            try {
                                image = reader.acquireLatestImage();
                                if (image == null) {
                                    return;
                                }
                                pushNv12(image);
                            } catch (Throwable t) {
                                Log.w(TAG, "onImageAvailable", t);
                            } finally {
                                if (image != null) {
                                    image.close();
                                }
                            }
                        }
                    },
                    backgroundHandler);

            final String openId = id;
            try {
                manager.openCamera(
                        openId,
                        new CameraDevice.StateCallback() {
                            @Override
                            public void onOpened(CameraDevice camera) {
                                synchronized (LOCK) {
                                    cameraDevice = camera;
                                    createSessionLocked(camera);
                                }
                            }

                            @Override
                            public void onDisconnected(CameraDevice camera) {
                                Log.w(TAG, "camera disconnected");
                                synchronized (LOCK) {
                                    camera.close();
                                    if (cameraDevice == camera) {
                                        cameraDevice = null;
                                    }
                                    running = false;
                                }
                                onCameraState(false, "disconnected");
                            }

                            @Override
                            public void onError(CameraDevice camera, int error) {
                                Log.e(TAG, "camera error " + error);
                                synchronized (LOCK) {
                                    camera.close();
                                    if (cameraDevice == camera) {
                                        cameraDevice = null;
                                    }
                                    running = false;
                                }
                                onCameraState(false, "error " + error);
                            }
                        },
                        backgroundHandler);
            } catch (SecurityException e) {
                Log.e(TAG, "openCamera security", e);
                stopLocked();
                onCameraState(false, "permission denied");
            } catch (CameraAccessException e) {
                Log.e(TAG, "openCamera", e);
                stopLocked();
                onCameraState(false, e.getMessage());
            }
        }
    }

    private static void createSessionLocked(CameraDevice camera) {
        final ImageReader reader = imageReader;
        if (reader == null) {
            onCameraState(false, "no ImageReader");
            return;
        }
        final Surface surface = reader.getSurface();
        try {
            camera.createCaptureSession(
                    Collections.singletonList(surface),
                    new CameraCaptureSession.StateCallback() {
                        @Override
                        public void onConfigured(CameraCaptureSession session) {
                            synchronized (LOCK) {
                                if (cameraDevice == null) {
                                    return;
                                }
                                captureSession = session;
                                try {
                                    CaptureRequest.Builder builder =
                                            cameraDevice.createCaptureRequest(
                                                    CameraDevice.TEMPLATE_PREVIEW);
                                    builder.addTarget(surface);
                                    builder.set(
                                            CaptureRequest.CONTROL_AF_MODE,
                                            CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO);
                                    session.setRepeatingRequest(
                                            builder.build(), null, backgroundHandler);
                                    running = true;
                                    onCameraState(true, "live");
                                    Log.i(TAG, "capture session live");
                                } catch (CameraAccessException e) {
                                    Log.e(TAG, "setRepeatingRequest", e);
                                    onCameraState(false, e.getMessage());
                                }
                            }
                        }

                        @Override
                        public void onConfigureFailed(CameraCaptureSession session) {
                            Log.e(TAG, "capture session configure failed");
                            onCameraState(false, "configure failed");
                        }
                    },
                    backgroundHandler);
        } catch (CameraAccessException e) {
            Log.e(TAG, "createCaptureSession", e);
            onCameraState(false, e.getMessage());
        }
    }

    private static void pushNv12(Image image) {
        if (image.getFormat() != ImageFormat.YUV_420_888) {
            return;
        }
        Image.Plane[] planes = image.getPlanes();
        if (planes.length < 3) {
            return;
        }
        int width = image.getWidth();
        int height = image.getHeight();
        Image.Plane yPlane = planes[0];
        Image.Plane uPlane = planes[1];
        Image.Plane vPlane = planes[2];
        int yStride = yPlane.getRowStride();
        int uvStride = uPlane.getRowStride();
        int uvPixelStride = uPlane.getPixelStride();
        int uvHeight = height / 2;

        ByteBuffer yBuf = yPlane.getBuffer();
        int ySize = yStride * height;
        byte[] yData = new byte[ySize];
        yBuf.position(0);
        int yCopy = Math.min(ySize, yBuf.remaining());
        yBuf.get(yData, 0, yCopy);

        byte[] uvData;
        int outUvStride;
        if (uvPixelStride == 2) {
            // Semi-planar: U plane buffer is UVUV… (NV12-style on most devices).
            ByteBuffer uvBuf = uPlane.getBuffer();
            int uvSize = uvStride * uvHeight;
            uvData = new byte[uvSize];
            uvBuf.position(0);
            int uvCopy = Math.min(uvSize, uvBuf.remaining());
            uvBuf.get(uvData, 0, uvCopy);
            outUvStride = uvStride;
        } else {
            // Planar I420 → interleave U+V into NV12.
            ByteBuffer uBuf = uPlane.getBuffer();
            ByteBuffer vBuf = vPlane.getBuffer();
            int uvWidth = width / 2;
            outUvStride = width;
            uvData = new byte[outUvStride * uvHeight];
            for (int row = 0; row < uvHeight; row++) {
                for (int col = 0; col < uvWidth; col++) {
                    int srcIdx = row * uPlane.getRowStride() + col * uvPixelStride;
                    int dstIdx = row * outUvStride + col * 2;
                    uvData[dstIdx] = uBuf.get(srcIdx);
                    uvData[dstIdx + 1] = vBuf.get(srcIdx);
                }
            }
        }

        int rotationDegrees = computeRotationDegrees();
        onNv12Frame(yData, uvData, width, height, yStride, outUvStride, rotationDegrees);
        long n = ++frameCount;
        if (n == 1L || n == 30L || n == 300L) {
            Log.i(
                    TAG,
                    "nv12 frame #"
                            + n
                            + " "
                            + width
                            + "x"
                            + height
                            + " yStride="
                            + yStride
                            + " uvStride="
                            + outUvStride
                            + " uvPixelStride="
                            + uvPixelStride
                            + " rot="
                            + rotationDegrees);
        }
    }

    /**
     * CW degrees to rotate sensor buffers so content is upright for the
     * current display orientation (Camera2 JPEG_ORIENTATION formula).
     */
    private static int computeRotationDegrees() {
        int device = displayRotationDegrees(hostActivity);
        if (frontFacing) {
            return (sensorOrientationDeg + device) % 360;
        }
        return (sensorOrientationDeg - device + 360) % 360;
    }

    private static int displayRotationDegrees(Activity activity) {
        if (activity == null) {
            return 0;
        }
        try {
            int rotation;
            if (Build.VERSION.SDK_INT >= 30) {
                Display display = activity.getDisplay();
                rotation = display != null ? display.getRotation() : Surface.ROTATION_0;
            } else {
                @SuppressWarnings("deprecation")
                Display display = activity.getWindowManager().getDefaultDisplay();
                rotation = display.getRotation();
            }
            switch (rotation) {
                case Surface.ROTATION_90:
                    return 90;
                case Surface.ROTATION_180:
                    return 180;
                case Surface.ROTATION_270:
                    return 270;
                case Surface.ROTATION_0:
                default:
                    return 0;
            }
        } catch (Throwable t) {
            Log.w(TAG, "displayRotationDegrees", t);
            return 0;
        }
    }

    private static String resolveCameraId(CameraManager manager, String requested)
            throws CameraAccessException {
        String[] ids = manager.getCameraIdList();
        if (ids.length == 0) {
            return null;
        }
        String want = requested == null ? "" : requested.trim();
        if (!want.isEmpty()) {
            for (String id : ids) {
                if (id.equals(want)) {
                    return id;
                }
            }
            Integer facingWant = null;
            if ("front".equalsIgnoreCase(want)) {
                facingWant = CameraCharacteristics.LENS_FACING_FRONT;
            } else if ("back".equalsIgnoreCase(want)) {
                facingWant = CameraCharacteristics.LENS_FACING_BACK;
            }
            if (facingWant != null) {
                for (String id : ids) {
                    Integer facing =
                            manager.getCameraCharacteristics(id)
                                    .get(CameraCharacteristics.LENS_FACING);
                    if (facingWant.equals(facing)) {
                        return id;
                    }
                }
            }
        }
        // Prefer front for self-view, then first listed.
        for (String id : ids) {
            Integer facing =
                    manager.getCameraCharacteristics(id).get(CameraCharacteristics.LENS_FACING);
            if (facing != null && facing == CameraCharacteristics.LENS_FACING_FRONT) {
                return id;
            }
        }
        return ids[0];
    }

    private static Size chooseSize(CameraManager manager, String cameraId) {
        try {
            CameraCharacteristics chars = manager.getCameraCharacteristics(cameraId);
            android.hardware.camera2.params.StreamConfigurationMap map =
                    chars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP);
            if (map == null) {
                return new Size(TARGET_WIDTH, TARGET_HEIGHT);
            }
            Size[] sizes = map.getOutputSizes(ImageFormat.YUV_420_888);
            if (sizes == null || sizes.length == 0) {
                return new Size(TARGET_WIDTH, TARGET_HEIGHT);
            }
            Size best = sizes[0];
            long bestScore = Long.MAX_VALUE;
            for (Size s : sizes) {
                long dw = (long) s.getWidth() - TARGET_WIDTH;
                long dh = (long) s.getHeight() - TARGET_HEIGHT;
                long score = dw * dw + dh * dh;
                // Prefer sizes at or above target when scores are close.
                if (s.getWidth() < TARGET_WIDTH / 2 || s.getHeight() < TARGET_HEIGHT / 2) {
                    score += 10_000_000L;
                }
                if (score < bestScore) {
                    bestScore = score;
                    best = s;
                }
            }
            return best;
        } catch (Throwable t) {
            Log.w(TAG, "chooseSize fallback", t);
            return new Size(TARGET_WIDTH, TARGET_HEIGHT);
        }
    }

    private static void stopLocked() {
        running = false;
        hostActivity = null;
        if (captureSession != null) {
            try {
                captureSession.close();
            } catch (Throwable ignored) {
            }
            captureSession = null;
        }
        if (cameraDevice != null) {
            try {
                cameraDevice.close();
            } catch (Throwable ignored) {
            }
            cameraDevice = null;
        }
        if (imageReader != null) {
            try {
                imageReader.close();
            } catch (Throwable ignored) {
            }
            imageReader = null;
        }
        stopBackgroundThread();
    }

    private static void startBackgroundThread() {
        stopBackgroundThread();
        backgroundThread = new HandlerThread("sleek-camera");
        backgroundThread.start();
        backgroundHandler = new Handler(backgroundThread.getLooper());
    }

    private static void stopBackgroundThread() {
        if (backgroundThread != null) {
            backgroundThread.quitSafely();
            try {
                backgroundThread.join(1000);
            } catch (InterruptedException ignored) {
            }
            backgroundThread = null;
            backgroundHandler = null;
        }
    }
}
