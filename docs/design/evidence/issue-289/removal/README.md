# Issue #289 removal evidence

The installed Corral.app was rebuilt from this checkout and launched. The
window capture attempt was blocked by the headless lane/display environment:
`screencapture -x /tmp/g289-board.png` returned `could not create image from display`.

The runtime binary was nevertheless built and installed with:

    cargo build --release -p corrald-ui
    CORRAL_SKIP_CODESIGN=1 scripts/install-corral-ui.sh
    open /Applications/Corral.app

The board implementation no longer renders the fleet-health strip; its first
content is the existing search/filter/agent board.
