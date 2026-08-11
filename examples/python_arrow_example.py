import pyarrow as pa
from aaronia import AaroniaConfig, AaroniaSource

def main():
    print("Initializing Aaronia SDR...")
    config = AaroniaConfig()
    config.center_freq = 2.4e9  # 2.4 GHz
    config.sample_rate = 10e6   # 10 MHz
    config.format = "F32"

    source = AaroniaSource()
    source.start_streaming(config)

    print("Streaming started.")
    
    try:
        # Read 1024 samples as an Arrow array
        print("Reading 1024 samples (Arrow format)...")
        arrow_data = source.read_samples_arrow(1024)
        
        # Cast to pyarrow array for inspection
        array = pa.array(arrow_data)
        
        print(f"Read {len(array)} IQ pairs.")
        print(f"Data type: {array.type}")
        print("First 5 samples:")
        for i in range(min(5, len(array))):
            print(f"  {array[i]}")

        drops = source.cumulative_drops()
        if drops > 0:
            print(f"Warning: {drops} drops detected!")
            
    finally:
        print("Stopping streaming...")
        source.stop_streaming()

if __name__ == "__main__":
    main()
