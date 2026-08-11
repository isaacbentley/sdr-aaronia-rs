import SoapySDR
from SoapySDR import *
import numpy as np

def main():
    print("Enumerating SoapySDR devices...")
    results = SoapySDR.Device.enumerate()
    
    # Check if aaronia driver is present
    aaronia_found = False
    for res in results:
        if res.get("driver") == "aaronia":
            aaronia_found = True
            print("Found Aaronia device:", dict(res))
            break
            
    if not aaronia_found:
        print("Aaronia SoapySDR driver not found. Is the plugin installed?")
        print("Available drivers:")
        for res in results:
            print(" -", dict(res))
        return

    print("Opening Aaronia SDR...")
    sdr = SoapySDR.Device(dict(driver="aaronia"))

    print("Configuring SDR...")
    # Set sample rate (e.g. 20 MHz)
    sdr.setSampleRate(SOAPY_SDR_RX, 0, 20e6)
    
    # Set center frequency (e.g. 2.4 GHz)
    sdr.setFrequency(SOAPY_SDR_RX, 0, 2.4e9)
    
    # Print stream formats
    formats = sdr.getStreamFormats(SOAPY_SDR_RX, 0)
    print(f"Supported RX formats: {formats}")

    print("Setting up RX stream (CF32)...")
    rx_stream = sdr.setupStream(SOAPY_SDR_RX, SOAPY_SDR_CF32)
    sdr.activateStream(rx_stream)
    print("Streaming started.")

    # Read some samples
    try:
        buff = np.zeros(1024, dtype=np.complex64)
        print("Reading 1024 samples...")
        sr = sdr.readStream(rx_stream, [buff], len(buff))
        print(f"Read {sr.ret} samples.")
        print(f"Flags: {sr.flags}, TimeNs: {sr.timeNs}")
        if sr.ret > 0:
            print("First 5 samples:")
            for i in range(min(5, sr.ret)):
                print(f"  {buff[i]}")
    finally:
        print("Deactivating and closing stream...")
        sdr.deactivateStream(rx_stream)
        sdr.closeStream(rx_stream)

if __name__ == "__main__":
    main()
