#!/usr/bin/env python3
"""M9 dry-run qa-sip far-end probe (runs inside the vs-harness image).

Joins the qa-sip Mumble channel as a controlled, DETERMINISTIC far end:
  * TRANSMITS a steady 660 Hz tone into qa-sip  -> proves qa-sip -> A
    (the SIP caller's RTP RX should show a 660 Hz component).
  * RECEIVES channel audio and FFT-detects 440 Hz -> proves A -> qa-sip
    (the caller emits 440 Hz; it arrives via the ryan-phone bridge).

Named 'm9-probe' (matches the streaming agent's non-human denylist, so it does
NOT itself register as a human presence — only the ryan-phone bridge does).
"""
import os, sys, time, threading, math
import numpy as np
import pymumble_py3 as pymumble
from pymumble_py3.constants import PYMUMBLE_CLBK_SOUNDRECEIVED

HOST=os.getenv("MUMBLE_HOST","127.0.0.1"); PORT=int(os.getenv("MUMBLE_PORT","64738"))
PASS=os.getenv("MUMBLE_PASSWORD",""); CH=os.getenv("MUMBLE_CHANNEL","qa-sip")
USER=os.getenv("PROBE_NAME","m9-probe"); SECS=float(os.getenv("PROBE_SECS","16"))
TX_HZ=int(os.getenv("TX_HZ","660"))
def log(*a): print("[probe]",*a,flush=True)

rx=[]  # accumulate received 48k int16
def on_sound(user, chunk):
    a=np.frombuffer(chunk.pcm, dtype="<i2")
    rx.append((user.get("name","?"), a))

m=pymumble.Mumble(HOST, user=USER, port=PORT, password=PASS, reconnect=False)
m.callbacks.set_callback(PYMUMBLE_CLBK_SOUNDRECEIVED, on_sound)
m.set_receive_sound(True)
m.start(); m.is_ready()
ch=m.channels.find_by_name(CH); ch.move_in()
time.sleep(0.3)
log(f"joined '{CH}' as {USER} (session {m.users.myself['session']}); TX {TX_HZ}Hz for {SECS}s")

stop=threading.Event()
def tx_loop():
    # 48k int16 660Hz, 20ms (960 samples) chunks in real time
    n=960; t=0
    while not stop.is_set():
        idx=np.arange(t,t+n)
        s=(0.5*32767*np.sin(2*math.pi*TX_HZ*idx/48000.0)).astype("<i2")
        m.sound_output.add_sound(s.tobytes()); t+=n
        time.sleep(0.02)
threading.Thread(target=tx_loop,daemon=True).start()

time.sleep(SECS)
stop.set(); time.sleep(0.3)

# analyze received audio for 440 Hz (caller's tone)
if rx:
    who=set(n for n,_ in rx)
    allpcm=np.concatenate([a for _,a in rx]).astype(np.float64)
    allpcm-=allpcm.mean()
    N=len(allpcm); f=np.fft.rfftfreq(N,1/48000.0)
    X=np.abs(np.fft.rfft(allpcm*np.hanning(N))); tot=float(X.sum())+1e-9
    def band(f0):
        msk=(f>=f0-25)&(f<=f0+25); return float(X[msk].max())/tot*1e4 if msk.any() else 0.0
    res=(f"PROBE_RESULT samples={N} ratio440={band(440):.2f} ratio660={band(660):.2f} "
         f"users={sorted(who)}")
    print(res)
    open("/probe/probe_result.txt","w").write(res+"\n")
else:
    res="PROBE_RESULT samples=0 ratio440=0.00 users=[] (NO audio received)"
    print(res); open("/probe/probe_result.txt","w").write(res+"\n")
m.stop()
