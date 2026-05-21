/**
 * SSO authentication client
 */

import axios, { AxiosInstance } from 'axios';
import { SsoUserInfo, SsoProviderConfig, TokenResponse } from './models';

export class SsoClient {
  private axios: AxiosInstance;

  constructor(baseUrl: string, apiKey?: string, timeout: number = 30000) {
    this.axios = axios.create({
      baseURL: baseUrl,
      timeout,
      headers: {
        'Content-Type': 'application/json',
        ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
      },
    });
  }

  async listProviders(): Promise<SsoProviderConfig[]> {
    const response = await this.axios.get<SsoProviderConfig[]>(
      '/api/v1/sso/providers'
    );
    return response.data;
  }

  async getProvider(providerId: string): Promise<SsoProviderConfig> {
    const response = await this.axios.get<SsoProviderConfig>(
      `/api/v1/sso/providers/${providerId}`
    );
    return response.data;
  }

  async createProvider(config: SsoProviderConfig): Promise<SsoProviderConfig> {
    const response = await this.axios.post<SsoProviderConfig>(
      '/api/v1/sso/providers',
      config
    );
    return response.data;
  }

  async updateProvider(
    providerId: string,
    config: SsoProviderConfig
  ): Promise<SsoProviderConfig> {
    const response = await this.axios.put<SsoProviderConfig>(
      `/api/v1/sso/providers/${providerId}`,
      config
    );
    return response.data;
  }

  async deleteProvider(providerId: string): Promise<void> {
    await this.axios.delete(`/api/v1/sso/providers/${providerId}`);
  }

  async getAuthorizationUrl(providerId: string, redirectUri: string): Promise<string> {
    const response = await this.axios.get<{ url: string }>(
      `/api/v1/sso/providers/${providerId}/authorize`,
      {
        params: { redirect_uri: redirectUri },
      }
    );
    return response.data.url;
  }

  async exchangeCode(
    providerId: string,
    code: string,
    redirectUri: string
  ): Promise<TokenResponse> {
    const response = await this.axios.post<TokenResponse>(
      `/api/v1/sso/providers/${providerId}/token`,
      {
        code,
        redirect_uri: redirectUri,
      }
    );
    return response.data;
  }

  async getUserInfo(providerId: string, accessToken: string): Promise<SsoUserInfo> {
    const response = await this.axios.get<SsoUserInfo>(
      `/api/v1/sso/providers/${providerId}/userinfo`,
      {
        headers: { Authorization: `Bearer ${accessToken}` },
      }
    );
    return response.data;
  }

  async validateToken(providerId: string, token: string): Promise<SsoUserInfo> {
    const response = await this.axios.post<SsoUserInfo>('/api/v1/sso/validate', {
      provider_id: providerId,
      token,
    });
    return response.data;
  }
}