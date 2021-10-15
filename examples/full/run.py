import sys
import os
print(sys.path)
import pandas as pd
print('pandas', pd.__version__)
import dppd
print(sys.modules['dppd'])
print('venv', os.listdir("/anysnake2/venv"))
print('linked-in', os.listdir("/anysnake2/venv/linked_in"))
print('mbf-r', os.listdir("/anysnake2/venv/linked_in/mbf-r/src"))
import mbf_sampledata
