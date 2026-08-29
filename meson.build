project('sleek', 'c', version: '0.1.0', meson_version: '>=1.9')

gtk_dep = dependency('gtk4')
adw_dep = dependency('libadwaita-1')

configure_file(
  input: 'assets/uk.nandi.sleek.metainfo.xml',
  output: 'uk.nandi.sleek.metainfo.xml',
  copy: true,
)

rust_target = ''
build_deps = []
extra_env = environment()

if meson.is_cross_build()
  cpu_family = host_machine.cpu_family()
  if cpu_family == 'aarch64'
    rust_target = 'aarch64-linux-android'
  elif cpu_family == 'x86_64'
    rust_target = 'x86_64-linux-android'
  else
    error('unsupported Android ABI: @0@'.format(cpu_family))
  endif

  build_deps += subproject('gtk').get_variable('libgtk')
  build_deps += subproject('libadwaita').get_variable('libadwaita')
  android_cc = meson.get_compiler('c').cmd_array()[0]
  linker_env = 'CARGO_TARGET_' + rust_target.to_upper().replace('-', '_') + '_LINKER'
  extra_env.set(linker_env, android_cc)
  extra_env.set('PKG_CONFIG_PATH', meson.global_build_root() / 'meson-uninstalled')
  extra_env.set('PKG_CONFIG_ALLOW_CROSS', '1')

  # C++ deps in the crate graph link the NDK's shared C++ runtime, which
  # Android does not ship. Package it or dlopen of our library fails with
  # 'library "libc++_shared.so" not found'.
  fs = import('fs')
  ndk_sysroot_libs = fs.parent(fs.parent(android_cc)) / 'sysroot/usr/lib' / rust_target
  custom_target(
    'libc++_shared',
    output: 'libc++_shared.so',
    command: ['cp', ndk_sysroot_libs / 'libc++_shared.so', '@OUTPUT@'],
    install: true,
    install_dir: get_option('libdir'),
    install_tag: 'runtime',
  )
endif

profile = get_option('buildtype') == 'release' ? 'release' : 'dev'

custom_target(
  'sleek_relm4',
  output: 'libsleek_relm4.so',
  console: true,
  build_always_stale: true,
  depends: build_deps,
  env: extra_env,
  command: [
    find_program('scripts/cargo-build-relm4-cdylib.sh'),
    '--manifest-path', meson.project_source_root() / 'host/Cargo.toml',
    '--target-dir', meson.current_build_dir() / 'cargo-target',
    '--out', '@OUTPUT@',
    '--rust-target', rust_target,
    '--profile', profile,
  ],
  install: true,
  install_dir: get_option('libdir'),
  install_tag: 'runtime',
)
