import pandas as pd
import numpy as np
from datetime import datetime

# Read the CSV file
df = pd.read_csv('sales_data.csv')

print("Original data shape:", df.shape)
print("\nOriginal data:")
print(df.head())

# Check for missing values
print("\nMissing values per column:")
print(df.isnull().sum())

# Remove rows with any missing values
df_cleaned = df.dropna()
print("\nData shape after removing missing values:", df_cleaned.shape)

# Convert quarter to date format (YYYY-MM-DD)
# Assuming the data is from 2024
quarter_to_date = {
    'Q1': '2024-01-01',
    'Q2': '2024-04-01', 
    'Q3': '2024-07-01',
    'Q4': '2024-10-01'
}

df_cleaned['date'] = df_cleaned['quarter'].map(quarter_to_date)

# Reorder columns to put date first, then remove the quarter column
cols = ['date'] + [col for col in df_cleaned.columns if col not in ['date', 'quarter']]
df_cleaned = df_cleaned[cols]

print("\nCleaned data with date column:")
print(df_cleaned.head())

# Save the cleaned data
df_cleaned.to_csv('cleaned_data.csv', index=False)
print("\nData saved to cleaned_data.csv")