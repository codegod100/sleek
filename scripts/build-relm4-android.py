#!/usr/bin/env python3
"""Build Sleek's Relm4 APK without Pixiewood's Perl frontend."""
import argparse, configparser, os, secrets, shutil, subprocess, tarfile
from pathlib import Path

# The production APK targets physical Android devices. Keeping this arm64-only
# also avoids building a second complete GTK stack for emulator-only x86_64.
ARCHES = {"aarch64": "arm64-v8a"}

def run(*args, cwd=None, env=None):
    print("+", *args, flush=True)
    subprocess.run([str(a) for a in args], cwd=cwd, env=env, check=True)

def android_home(value):
    if value:
        explicit = Path(value).expanduser().resolve()
        if (explicit / "platforms").is_dir() and (explicit / "build-tools").is_dir():
            return explicit
        raise SystemExit(f"Android SDK is incomplete: {explicit}")
    sdkmanager = shutil.which("sdkmanager")
    commandline_tools_sdk = (
        Path(sdkmanager).resolve().parents[3] if sdkmanager else None
    )
    for item in (os.getenv("ANDROID_HOME"), os.getenv("ANDROID_SDK_ROOT"),
                 commandline_tools_sdk, Path.home()/".local/share/android-sdk",
                 Path.home()/"Android/Sdk"):
        if item and (Path(item).expanduser()/"platform-tools/adb").is_file():
            return Path(item).expanduser().resolve()
    raise SystemExit("Android SDK not found; set ANDROID_HOME or pass --android-home")

def ndk_home(sdk, value):
    if value:
        return Path(value).expanduser().resolve()
    found = sorted((sdk/"ndk").glob("*"))
    if not found:
        raise SystemExit(f"Android NDK not found under {sdk/'ndk'}")
    return found[-1]

def prepare(root, builder, ndk, release):
    state = root/".relm4-android"; state.mkdir(exist_ok=True)
    llvm = ndk/"toolchains/llvm/prebuilt/linux-x86_64"
    if not llvm.is_dir(): raise SystemExit(f"incomplete NDK: {llvm} is missing")
    cross = state/"toolchain.cross"
    cross.write_text(f"""[constants]
toolchain = '{llvm}/'
[binaries]
cmake = 'cmake'
[properties]
cmake_toolchain_file = '{ndk}/build/cmake/android.toolchain.cmake'
""")
    sub = root/"subprojects"; sub.mkdir(exist_ok=True)
    for dep, patches in {"glib":["hack"],"fontconfig":[],"gtk":[],"libadwaita":[]}.items():
        source = builder/"prepare/wraps"/dep
        cfg = configparser.ConfigParser(interpolation=None); cfg.optionxform = str
        cfg.read(source/f"{dep}.wrap")
        if patches:
            dest = sub/"packagefiles"/dep; dest.mkdir(parents=True, exist_ok=True)
            for patch in patches: shutil.copy2(source/f"{patch}.patch", dest)
            for section in ("wrap-file","wrap-git","wrap-hg","wrap-svn"):
                if cfg.has_section(section):
                    cfg[section]["diff_files"] = ",".join(f"{dep}/{p}.patch" for p in patches)
        with (sub/f"{dep}.wrap").open("w") as out: cfg.write(out, space_around_delimiters=False)
    for arch in ARCHES:
        build = state/f"bin-{arch}"
        args = ["meson","setup"]
        if build.exists(): args.append("--reconfigure")
        args += ["--cross-file",cross,"--cross-file",builder/"prepare/arch"/f"{arch}.cross",
                 "--cross-file",builder/"prepare/android.cross",
                 "--buildtype","release" if release else "debug"]
        if release: args.append("--strip")
        run(*args, build, root)
    return state

def put(path, text):
    path.parent.mkdir(parents=True, exist_ok=True); path.write_text(text)

def generate(root, builder, state):
    out = state/"android"
    if out.exists(): shutil.rmtree(out)
    out.mkdir()
    with tarfile.open(builder/"generate/skel.tar") as archive:
        archive.extractall(out, filter="data")
    put(out/"settings.gradle", """pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS); repositories { google(); mavenCentral() } }
rootProject.name='Sleek'; include ':app'
""")
    abis = ", ".join(f"'{a}'" for a in ARCHES.values())
    put(out/"app/build.gradle", f"""plugins {{ alias(libs.plugins.android.application) }}
android {{
 namespace='uk.nandi.sleek'; compileSdk {{ version=release(35) }}
 signingConfigs {{ sleek {{ storeFile file('{root / "android/ci.keystore"}'); storePassword 'android'; keyAlias 'androiddebugkey'; keyPassword 'android' }} }}
 defaultConfig {{ applicationId 'uk.nandi.sleek'; minSdk 31; targetSdk 35; versionCode 16777474; versionName '0.1.2'; ndk {{ abiFilters {abis} }} }}
 buildTypes {{ debug {{ minifyEnabled false; signingConfig signingConfigs.sleek }}; release {{ minifyEnabled false; signingConfig signingConfigs.sleek }} }}
 compileOptions {{ sourceCompatibility JavaVersion.VERSION_11; targetCompatibility JavaVersion.VERSION_11 }}
 enableKotlin=false
 splits {{ abi {{ enable=true; reset(); include {abis}; universalApk=true }} }}
}}
dependencies {{ implementation libs.androidx.annotation }}
""")
    put(out/"app/src/main/AndroidManifest.xml", """<manifest xmlns:android="http://schemas.android.com/apk/res/android">
<uses-permission android:name="android.permission.INTERNET"/><uses-permission android:name="android.permission.ACCESS_NETWORK_STATE"/>
<application android:name="org.gtk.android.RuntimeApplication" android:icon="@mipmap/ic_launcher" android:label="@string/app_name" android:theme="@style/Theme.Gtk">
<meta-data android:name="gtk.android.lib_name" android:value="sleek_relm4"/>
<activity android:name="org.gtk.android.ToplevelActivity" android:configChanges="density|orientation|screenLayout|screenSize|touchscreen|uiMode" android:windowSoftInputMode="adjustResize" android:theme="@style/Theme.GtkSurface" android:exported="true">
<intent-filter><action android:name="android.intent.action.MAIN"/><category android:name="android.intent.category.LAUNCHER"/></intent-filter>
<intent-filter><action android:name="android.intent.action.VIEW"/><category android:name="android.intent.category.DEFAULT"/><category android:name="android.intent.category.BROWSABLE"/><data android:scheme="freeq"/></intent-filter>
</activity></application></manifest>""")
    put(out/"app/src/main/res/values/strings.xml", '<resources><string name="app_name">Sleek</string></resources>')
    for qual, colors in {"values":("#FFFFFFFF","#FFFAFAFA","#CC000000"),"values-night":("#FF1E1E1E","#FF242424","#FFFFFFFF")}.items():
        put(out/f"app/src/main/res/{qual}/colors.xml", f'<resources><color name="base">{colors[0]}</color><color name="bg">{colors[1]}</color><color name="fg">{colors[2]}</color><color name="accent">#FF3584E4</color></resources>')
    icon = root/"assets/icons/uk.nandi.sleek-256.png"
    for density in ("mdpi","hdpi","xhdpi","xxhdpi"):
        target=out/f"app/src/main/res/mipmap-{density}/ic_launcher.png"; target.parent.mkdir(parents=True,exist_ok=True); shutil.copy2(icon,target)
    java=root/"subprojects/gtk/gdk/android/glue/java/org/gtk/android"
    target=out/"app/src/main/java/org/gtk/android"; target.parent.mkdir(parents=True,exist_ok=True)
    if not java.is_dir(): raise RuntimeError(f"GTK Java glue missing: {java}")
    target.symlink_to(java.resolve(), target_is_directory=True)
    return out

def build(state, project, sdk, release):
    staged=state/"root"
    if staged.exists(): shutil.rmtree(staged)
    jobs=max(1,(os.cpu_count() or 2)//2)
    for arch in ARCHES:
        directory=state/f"bin-{arch}"; run("ninja","-j",jobs,"-C",directory)
        run("meson","install","-C",directory,"--destdir",staged,"--tags","runtime")
    assets=project/"app/src/main/assets"; assets.mkdir(parents=True,exist_ok=True)
    for child in staged.iterdir():
        if child.name not in ("bin","lib"): shutil.copytree(child,assets/child.name,dirs_exist_ok=True)
    schemas=staged/"share/glib-2.0/schemas"
    if schemas.is_dir(): run("glib-compile-schemas","--targetdir",assets/"share/glib-2.0/schemas",schemas)
    (assets/"afpr").write_bytes(secrets.token_bytes(128))
    jni=project/"app/src/main/jniLibs"; jni.symlink_to((staged/"lib").resolve(),target_is_directory=True)
    env=os.environ.copy(); env["ANDROID_HOME"]=str(sdk)
    run("./gradlew","--no-daemon","assembleRelease" if release else "assembleDebug",cwd=project,env=env)
    apks=sorted((project/"app/build/outputs/apk").glob("**/*universal*.apk")) or sorted((project/"app/build/outputs/apk").glob("**/*.apk"))
    if not apks: raise RuntimeError("Gradle produced no APK")
    return apks[0]

def main():
    p=argparse.ArgumentParser(); p.add_argument("--android-home"); p.add_argument("--ndk"); p.add_argument("--release",action="store_true"); p.add_argument("--output",type=Path,default=Path("sleek-relm4.apk")); a=p.parse_args()
    root=Path(__file__).resolve().parent.parent; builder=root/"gtk-android-builder"; sdk=android_home(a.android_home); ndk=ndk_home(sdk,a.ndk)
    state=prepare(root,builder,ndk,a.release); project=generate(root,builder,state); apk=build(state,project,sdk,a.release)
    output=a.output.resolve(); output.parent.mkdir(parents=True,exist_ok=True); shutil.copy2(apk,output); print(output)
if __name__=="__main__": main()
