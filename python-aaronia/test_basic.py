import pytest
import numpy as np
import pyarrow as pa
from aaronia import AaroniaConfig, AaroniaSource

def test_config():
    config = AaroniaConfig()
    config.format = "F32"
    config.center_freq = 2400e6
    config.sample_rate = 20e6
    assert config is not None

def test_source_init():
    source = AaroniaSource()
    assert source is not None
