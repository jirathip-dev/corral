from browser import Browser
import time
with Browser() as b:
 b.load(params='preview=herd');print('initial',b.js('PROOF.state()'))
 b.call('Page.setWebLifecycleState',state='frozen');print('frozen',b.js('PROOF.state()'))
 b.call('Page.setWebLifecycleState',state='active');b.call('Page.bringToFront');print('active+front',b.js('PROOF.state()'))
 b.call('Emulation.setFocusEmulationEnabled',enabled=True);time.sleep(.2);print('focus emulation',b.js('PROOF.state()'))
 b.call('Emulation.setFocusEmulationEnabled',enabled=False);time.sleep(.2);print('focus reset',b.js('PROOF.state()'))
