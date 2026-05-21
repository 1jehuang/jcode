from setuptools import setup, find_packages

setup(
    name="jcode-sdk",
    version="0.12.0",
    description="Python SDK for JCode AI Code Assistant",
    long_description=open("README.md").read(),
    long_description_content_type="text/markdown",
    url="https://github.com/jcode-ai/jcode",
    author="JCode Team",
    author_email="dev@jcode.ai",
    license="MIT",
    packages=find_packages(),
    install_requires=[
        "requests >= 2.31.0",
        "websockets >= 12.0",
        "python-dateutil >= 2.8.2",
        "pydantic >= 2.0",
        "aiohttp >= 3.9.0",
    ],
    extras_require={
        "dev": [
            "pytest >= 7.0",
            "pytest-asyncio >= 0.21.0",
            "black >= 23.0",
            "isort >= 5.12.0",
        ]
    },
    classifiers=[
        "Development Status :: 4 - Beta",
        "Intended Audience :: Developers",
        "License :: OSI Approved :: MIT License",
        "Programming Language :: Python :: 3.8",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
    ],
    python_requires=">=3.8",
)