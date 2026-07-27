project(
  'flashcards',
  'rust',
  version: '0.1.0',
  meson_version: '>= 1.3.0',
)

gnome = import('gnome')

flashcards = custom_target(
  'flashcards',
  output: 'flashcards',
  command: [
    find_program('build-aux/build.sh'),
    meson.project_source_root(),
    '@OUTPUT@',
  ],
  depend_files: files(
    'Cargo.toml',
    'Cargo.lock',
    'src/lib.rs',
    'src/main.rs',
    'src/model.rs',
    'src/preferences.rs',
    'src/storage.rs',
    'src/study.rs',
    'src/ui.rs',
  ),
  build_by_default: true,
  install: true,
  install_dir: get_option('bindir'),
)

install_data(
  'data/page.polli.Flashcards.desktop',
  install_dir: get_option('datadir') / 'applications',
)
install_data(
  'data/page.polli.Flashcards.metainfo.xml',
  install_dir: get_option('datadir') / 'metainfo',
)
install_data(
  'data/deck.schema.json',
  install_dir: get_option('datadir') / 'flashcards',
)
install_data(
  'data/icons/hicolor/scalable/apps/page.polli.Flashcards.svg',
  install_dir: get_option('datadir') / 'icons/hicolor/scalable/apps',
)
install_data(
  'data/icons/hicolor/symbolic/apps/page.polli.Flashcards-symbolic.svg',
  install_dir: get_option('datadir') / 'icons/hicolor/symbolic/apps',
)

install_data(
  'data/page.polli.Flashcards.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0/schemas',
)

gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
